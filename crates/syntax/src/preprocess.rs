//! Preprocessor symbol definitions and include options.
//!
//! Shared directive events and legacy per-file conditional filtering.
//! Workspace-wide evaluation lives in `hir-expand::preprocess`; parsing keeps
//! the original CST for source editing and conditional balance checking.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default preprocessor symbols for the Sail compiler.
pub fn default_symbols() -> HashSet<String> {
    [
        "FEATURE_IMPLICITS",
        "FEATURE_CONSTANT_TYPES",
        "FEATURE_BITVECTOR_TYPE",
        "FEATURE_UNION_BARRIER",
        "FEATURE_STRICT_VAR",
        "FEATURE_STRICT_BITVECTOR",
        "FEATURE_STRICT_EXPONENTIALS",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// The program-wide vector index order set by `default Order dec | inc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefaultOrder {
    Inc,
    Dec,
}

impl DefaultOrder {
    /// Preprocessor symbols this order implies.
    ///
    /// Upstream defines `_DEFAULT_ORDER_SET` for any `default Order`, plus
    /// `_DEFAULT_DEC` or `_DEFAULT_INC` for the specific direction
    /// (preprocess.ml:292-301). The bundled prelude keys off these: without
    /// them every `$ifdef _DEFAULT_DEC` in `lib/vector.sail` silently takes
    /// the inc-order branch, and `lib/vector.sail:51` cannot raise its
    /// "a default order must be set" `$include_error`.
    pub fn symbols(self) -> [&'static str; 2] {
        match self {
            DefaultOrder::Dec => ["_DEFAULT_ORDER_SET", "_DEFAULT_DEC"],
            DefaultOrder::Inc => ["_DEFAULT_ORDER_SET", "_DEFAULT_INC"],
        }
    }

    /// Add this order's symbols to a preprocessor symbol set.
    pub fn apply(self, symbols: &mut HashSet<String>) {
        for s in self.symbols() {
            symbols.insert(s.to_owned());
        }
    }
}

/// Find the `default Order dec | inc` declaration in a parsed file, if any.
pub fn default_order_of_file(root: &crate::SyntaxNode) -> Option<DefaultOrder> {
    use parser::SyntaxKind as SK;
    for child in root.children() {
        if child.kind() != SK::DEFAULT_DEF {
            continue;
        }
        let mut saw_order = false;
        for tok in child.children_with_tokens().filter_map(|el| el.into_token()) {
            match tok.kind() {
                SK::KW_ORDER => saw_order = true,
                SK::KW_DEC if saw_order => return Some(DefaultOrder::Dec),
                SK::KW_INC if saw_order => return Some(DefaultOrder::Inc),
                _ => {}
            }
        }
    }
    None
}

/// Conditional-compilation state machine for `$ifdef` / `$ifndef` /
/// `$iftarget` / `$else` / `$endif`.
///
/// Mirrors upstream `Preprocess.cond_pragma` (preprocess.ml:102-128): the
/// branch not taken is scanned but discarded, and nesting is tracked so an
/// inner `$endif` does not close an outer conditional. Shared by ItemTree
/// lowering and top-level environment construction so both see the same set
/// of definitions.
#[derive(Debug, Default)]
pub struct CondFilter {
    stack: Vec<bool>,
}

impl CondFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the cursor is currently inside a taken branch.
    pub fn taking(&self) -> bool {
        self.stack.iter().all(|&t| t)
    }

    /// Feed one top-level node.
    ///
    /// Returns `true` when the node is a definition that survives conditional
    /// compilation. Directive nodes update the state and always return
    /// `false`, since they are not definitions themselves.
    ///
    /// `target` is the compilation target for `$iftarget`; `None` (the LSP
    /// case, and `sail --just-check`) always takes the `$else` branch, which
    /// is what upstream does when no target is selected.
    pub fn accept(
        &mut self,
        node: &crate::SyntaxNode,
        symbols: &mut HashSet<String>,
        target: Option<&str>,
    ) -> bool {
        use parser::SyntaxKind as SK;
        if node.kind() != SK::DIRECTIVE_DEF {
            return self.taking();
        }

        let raw = node.text().to_string();
        let text = strip_pragma_comment(&raw);

        if let Some(sym) = strip_directive(text, "define") {
            if self.taking() {
                symbols.insert(sym.to_owned());
            }
        } else if let Some(sym) = strip_directive(text, "ifdef") {
            let taking = symbols.contains(sym);
            self.stack.push(taking);
        } else if let Some(sym) = strip_directive(text, "ifndef") {
            let taking = !symbols.contains(sym);
            self.stack.push(taking);
        } else if text.starts_with("$iftarget") {
            let set = text.strip_prefix("$iftarget").unwrap_or("").trim();
            let taking = target.is_some_and(|t| set.split_whitespace().any(|s| s == t));
            self.stack.push(taking);
        } else if text == "$else" {
            if let Some(last) = self.stack.last_mut() {
                *last = !*last;
            }
        } else if text == "$endif" {
            self.stack.pop();
        }
        false
    }
}

/// A pragma's text with any trailing comment removed.
///
/// Upstream scans a pragma's argument with a dedicated lexer rule that ends
/// it at a line or block comment (lexer.mll:298-307), so
/// `$else // Concurrency Interface v1` is an ordinary `$else`. Comparing the
/// node's whole text against `"$else"` misses it, and then the enclosing
/// `$ifdef` never flips — silently dropping *both* branches, and with them
/// every `outcome` in lib/concurrency_interface/exception.sail.
pub fn strip_pragma_comment(text: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    let mut angle = false;
    let include = strip_directive(text.trim(), "include").is_some();
    for (i, &byte) in text.as_bytes().iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' && !angle {
            quoted = !quoted;
        }
        if include && !quoted && byte == b'<' {
            angle = true;
        }
        if include && !quoted && byte == b'>' {
            angle = false;
        }
        if !quoted
            && !angle
            && byte == b'/'
            && matches!(text.as_bytes().get(i + 1), Some(b'/') | Some(b'*'))
        {
            return text[..i].trim();
        }
    }
    text.trim()
}

/// Only events which can affect preprocessing. Offsets and function bodies
/// are deliberately absent, so ordinary edits backdate the workspace plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Define(String),
    Ifdef(String, bool),
    IfTarget(Vec<String>),
    TargetSet(String, Vec<String>),
    Else,
    Endif,
    Include { path: String, library: bool },
    Order(DefaultOrder),
    Other,
}

impl Event {
    pub fn from_node(node: &crate::SyntaxNode) -> Option<Self> {
        use parser::SyntaxKind as SK;
        if node.kind() == SK::DEFAULT_DEF {
            let mut order = false;
            for token in node.children_with_tokens().filter_map(|n| n.into_token()) {
                match token.kind() {
                    SK::KW_ORDER => order = true,
                    SK::KW_DEC if order => return Some(Self::Order(DefaultOrder::Dec)),
                    SK::KW_INC if order => return Some(Self::Order(DefaultOrder::Inc)),
                    _ => {}
                }
            }
            return None;
        }
        if node.kind() != SK::DIRECTIVE_DEF {
            return None;
        }
        let raw = node.text().to_string();
        let text = strip_pragma_comment(&raw);
        for (name, negative) in [("ifdef", false), ("ifndef", true)] {
            if let Some(arg) = strip_directive(text, name) {
                return Some(Self::Ifdef(arg.to_owned(), negative));
            }
        }
        if let Some(arg) = strip_directive(text, "define") {
            return Some(Self::Define(arg.into()));
        }
        if let Some(arg) = strip_directive(text, "iftarget") {
            return Some(Self::IfTarget(arg.split_whitespace().map(str::to_owned).collect()));
        }
        if let Some(arg) = strip_directive(text, "target_set") {
            let words: Vec<_> = arg.split_whitespace().map(str::to_owned).collect();
            if let Some((name, targets)) = words.split_first() {
                return Some(Self::TargetSet(name.clone(), targets.to_vec()));
            }
        }
        if text == "$else" {
            return Some(Self::Else);
        }
        if text == "$endif" {
            return Some(Self::Endif);
        }
        if let Some(arg) = strip_directive(text, "include") {
            for (start, end, library) in [('"', '"', false), ('<', '>', true)] {
                if let Some(path) = arg.strip_prefix(start).and_then(|s| s.strip_suffix(end)) {
                    return Some(Self::Include { path: path.into(), library });
                }
            }
        }
        Some(Self::Other)
    }

    pub fn consumed(&self) -> bool {
        matches!(
            self,
            Self::Define(_)
                | Self::Ifdef(..)
                | Self::IfTarget(_)
                | Self::TargetSet(..)
                | Self::Else
                | Self::Endif
        )
    }
}

/// Resolve exact paths; never guess by basename. Search paths are an LSP
/// extension used only after the compiler's normal relative/library path.
pub fn include_candidates(
    from: &Path,
    path: &str,
    library: bool,
    lib: Option<&Path>,
    search: &[PathBuf],
) -> Vec<PathBuf> {
    let first =
        if library { lib.map(|p| p.join(path)) } else { from.parent().map(|p| p.join(path)) };
    first
        .into_iter()
        .chain(search.iter().map(|p| p.join(path)))
        .map(|p| normalize_path(&p))
        .collect()
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// `$<name> <argument>` — returns the trimmed argument.
fn strip_directive<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let rest = text.strip_prefix('$')?.strip_prefix(name)?;
    // Require a separator so `$ifdefx` does not match `$ifdef`.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some(rest.trim())
}

#[cfg(test)]
mod cond_filter_tests {
    use super::*;

    fn directives(text: &str) -> Vec<String> {
        let (root, _) = crate::parse_text(text);
        let mut symbols = HashSet::new();
        let mut cond = CondFilter::new();
        root.children()
            .filter(|n| cond.accept(n, &mut symbols, None))
            .map(|n| n.text().to_string().trim().to_owned())
            .collect()
    }

    #[test]
    fn else_with_trailing_comment_still_flips() {
        let kept = directives(
            "$ifdef NOT_SET
val a : unit -> unit
$else // v1
val b : unit -> unit
$endif
",
        );
        assert_eq!(kept, ["val b : unit -> unit"]);
    }

    #[test]
    fn endif_with_trailing_comment_still_pops() {
        let kept = directives(
            "$ifdef NOT_SET
val a : unit -> unit
$endif /* done */
val c : unit -> unit
",
        );
        assert_eq!(kept, ["val c : unit -> unit"]);
    }
}

/// Callback type the preprocessor uses to read included files.
pub type IncludeReader = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// Options driving the preprocessor's `$include` resolution.
#[derive(Clone)]
pub struct PreprocessOptions {
    pub current_file_dir: Option<PathBuf>,
    pub search_dirs: Vec<PathBuf>,
    pub sail_dir: Option<PathBuf>,
    pub max_include_depth: usize,
    pub reader: IncludeReader,
    /// Current compilation target for `$iftarget` directives.
    ///
    /// - `Some("c")`: target-specific build, `$iftarget c` is taken
    /// - `None`: no compilation target; `$iftarget` takes its else branch.
    pub target: Option<String>,
}

impl Default for PreprocessOptions {
    fn default() -> Self {
        Self {
            current_file_dir: None,
            search_dirs: Vec::new(),
            sail_dir: None,
            max_include_depth: 32,
            reader: Arc::new(|_| None),
            target: None,
        }
    }
}

impl PreprocessOptions {
    /// Build a child options object for an included file.
    pub fn child_for_included(&self, included_path: &Path) -> Self {
        Self {
            current_file_dir: included_path.parent().map(Path::to_path_buf),
            search_dirs: self.search_dirs.clone(),
            sail_dir: self.sail_dir.clone(),
            max_include_depth: self.max_include_depth,
            reader: Arc::clone(&self.reader),
            target: self.target.clone(),
        }
    }
}

impl std::fmt::Debug for PreprocessOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreprocessOptions")
            .field("current_file_dir", &self.current_file_dir)
            .field("search_dirs", &self.search_dirs)
            .field("sail_dir", &self.sail_dir)
            .field("max_include_depth", &self.max_include_depth)
            .field("reader", &"<callback>")
            .finish()
    }
}
