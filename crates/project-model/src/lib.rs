//! Minimal `sail.proj` / `*.sail_project` parser.
//!
//! Parses the *ordered list of `.sail` files*
//! mentioned in `files` clauses, with the surrounding `directory`
//! prefix joined in. Module dependency edges, variables, and access
//! control are skipped for now — the LSP wants to know which files to
//! index, in what order, and that's the entire output of this pass.
//!
//! Conservative `if`/`then`/`else` handling: both branches are walked
//! and any files they mention are unioned in. The LSP indexes too
//! many files rather than too few; under-indexing would cause
//! cross-file goto-def to silently miss.
//!
//! Cycles in `directory` / `module` references can't happen because
//! this parser doesn't follow inter-module edges; it only collects
//! file leaves. Inputs that *also* contain a circular `requires`
//! graph are still processed correctly here — the dependency
//! validator that future stages add will need its own cycle check.

use std::path::{Path, PathBuf};

/// Parsed representation of a `.sail_project` / `sail.proj` file.
///
/// Contains the ordered file list (primary output) plus module
/// structure, variable definitions, and dependency declarations
/// extracted during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Files in declaration order, each as written in the project
    /// file (joined with the surrounding `directory` prefix when
    /// applicable). Paths are relative to the project file's parent
    /// directory; the caller is responsible for joining the project
    /// file's location to obtain absolute paths.
    pub files: Vec<PathBuf>,

    /// Variable definitions: `variable ARCH = A64`.
    pub variables: Vec<ProjectVariable>,

    /// Named module definitions with their files.
    pub modules: Vec<ProjectModule>,

    /// Inter-module dependency declarations.
    pub dependencies: Vec<ProjectDependency>,
}

/// A variable definition in a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectVariable {
    /// Variable name (e.g., `"ARCH"`).
    pub name: String,
    /// Variable value as a string (e.g., `"A64"`).
    pub value: String,
}

/// A named module in a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectModule {
    /// Module name (e.g., `"core"`, `"rv32"`).
    pub name: String,
    /// Files belonging to this module (with directory prefix applied).
    pub files: Vec<PathBuf>,
    /// Directory prefix for this module.
    pub directory: Option<String>,
}

/// An inter-module dependency declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDependency {
    /// The kind of dependency.
    pub kind: DependencyKind,
    /// The module that has the dependency (the one containing the declaration).
    pub from: String,
    /// The module being depended upon.
    pub to: String,
}

/// Kind of inter-module dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// `requires A` — module must be processed before this one.
    Requires,
    /// `after A` — ordering constraint (weaker than requires).
    After,
    /// `before A` — reverse ordering constraint.
    Before,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectParseError {
    /// Unexpected character at byte offset.
    UnexpectedChar(char, usize),
    /// Unterminated string starting at byte offset.
    UnterminatedString(usize),
    /// Unterminated block comment starting at byte offset.
    UnterminatedComment(usize),
    /// Unbalanced bracket: too many closers.
    UnbalancedClose(usize),
}

impl std::fmt::Display for ProjectParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectParseError::UnexpectedChar(c, offset) => {
                write!(f, "unexpected character `{c}` at offset {offset}")
            }
            ProjectParseError::UnterminatedString(offset) => {
                write!(f, "unterminated string starting at offset {offset}")
            }
            ProjectParseError::UnterminatedComment(offset) => {
                write!(f, "unterminated block comment starting at offset {offset}")
            }
            ProjectParseError::UnbalancedClose(offset) => {
                write!(f, "unbalanced closing bracket at offset {offset}")
            }
        }
    }
}

impl std::error::Error for ProjectParseError {}

/// Parse the textual contents of a `.sail_project` / `sail.proj` file
/// and return the ordered file list.
pub fn parse_project(source: &str) -> Result<ProjectFile, ProjectParseError> {
    let tokens = collapse_path_segments(lex(source)?);
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        files: Vec::new(),
        variables: Vec::new(),
        modules: Vec::new(),
        dependencies: Vec::new(),
        current_module: None,
    };
    parser.parse_top_level()?;
    let mut seen = std::collections::HashSet::new();
    let dedup = parser.files.into_iter().filter(|p| seen.insert(p.clone())).collect();
    Ok(ProjectFile {
        files: dedup,
        variables: parser.variables,
        modules: parser.modules,
        dependencies: parser.dependencies,
    })
}

/// Post-lex pass: fold `(Id `/`)* FileId` sequences into a single
/// `FileId` carrying the full path. The lexer alone produces
/// `Id("simple") Slash FileId("amod.sail")` for `simple/amod.sail`,
/// matching upstream Sail's grammar where the slash is parsed as a
/// `slash_exp` operator and the file portion is collected from the
/// rightmost `FileId`. For our purposes the easier transformation is
/// to glue them back into one `FileId` token before parsing, so the
/// rest of the parser only ever sees whole-path file ids.
fn collapse_path_segments(input: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        // Try to start a path: must be Id or `..`.
        let starts_path = matches!(input.get(i), Some(Tok::Id(_)) | Some(Tok::DotDot));
        if starts_path {
            // Walk forward greedily through `(Id|`..`) (Slash (Id|`..`))*`
            // and check whether it terminates at a `Slash FileId`.
            let mut j = i + 1;
            let mut path_parts: Vec<String> = match &input[i] {
                Tok::Id(s) => vec![s.clone()],
                Tok::DotDot => vec!["..".to_string()],
                _ => unreachable!(),
            };
            let mut consumed_to: Option<usize> = None;
            while matches!(input.get(j), Some(Tok::Slash)) {
                match input.get(j + 1) {
                    Some(Tok::Id(s)) => {
                        path_parts.push(s.clone());
                        j += 2;
                    }
                    Some(Tok::DotDot) => {
                        path_parts.push("..".to_string());
                        j += 2;
                    }
                    Some(Tok::FileId(name)) => {
                        path_parts.push(name.clone());
                        j += 2;
                        consumed_to = Some(j);
                        break;
                    }
                    _ => break,
                }
            }
            if let Some(end) = consumed_to {
                let joined = path_parts.join("/");
                out.push(Tok::FileId(joined));
                i = end;
                continue;
            }
        }
        out.push(input[i].clone());
        i += 1;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Bare identifier or one of the recognised keywords.
    Id(String),
    /// `name.ext` or `path/to/name.ext` token. Distinguished from a
    /// bare `Id` because it's the canonical "filename" form.
    FileId(String),
    /// Quoted string literal contents (without the surrounding `"`).
    Str(String),
    /// Variable reference `$name`.
    Var(String),
    LCurly,
    RCurly,
    LSquare,
    RSquare,
    LParen,
    RParen,
    Comma,
    Semi,
    Eq,
    /// Generic comparison / op token (`==`, `!=`, `<=`, `>=`, `<`,
    /// `>`). The MVP doesn't evaluate them but still needs to
    /// recognise the token shape so they don't get mistaken for
    /// something structural.
    OpCompare,
    /// `..` (parent reference in expressions).
    DotDot,
    /// `/` (path separator in expressions or division).
    Slash,
    /// Keyword tokens.
    KwFiles,
    KwDirectory,
    KwRequires,
    KwAfter,
    KwBefore,
    KwIf,
    KwThen,
    KwElse,
    KwTrue,
    KwFalse,
    KwVariable,
    KwTest,
}

fn keyword(id: &str) -> Option<Tok> {
    Some(match id {
        "files" => Tok::KwFiles,
        "directory" => Tok::KwDirectory,
        "requires" => Tok::KwRequires,
        "after" => Tok::KwAfter,
        "before" => Tok::KwBefore,
        "if" => Tok::KwIf,
        "then" => Tok::KwThen,
        "else" => Tok::KwElse,
        "true" => Tok::KwTrue,
        "false" => Tok::KwFalse,
        "variable" => Tok::KwVariable,
        "__test" => Tok::KwTest,
        _ => return None,
    })
}

fn lex(source: &str) -> Result<Vec<Tok>, ProjectParseError> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // Line comment.
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                // Block comment with nesting (matches upstream lexer).
                let start = i;
                i += 2;
                let mut depth = 1;
                while i < bytes.len() && depth > 0 {
                    if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if depth != 0 {
                    return Err(ProjectParseError::UnterminatedComment(start));
                }
            }
            b'/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            b'{' => {
                out.push(Tok::LCurly);
                i += 1;
            }
            b'}' => {
                out.push(Tok::RCurly);
                i += 1;
            }
            b'[' => {
                out.push(Tok::LSquare);
                i += 1;
            }
            b']' => {
                out.push(Tok::RSquare);
                i += 1;
            }
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b';' => {
                out.push(Tok::Semi);
                i += 1;
            }
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                out.push(Tok::OpCompare);
                i += 2;
            }
            b'=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            b'!' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                out.push(Tok::OpCompare);
                i += 2;
            }
            b'<' | b'>' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                    out.push(Tok::OpCompare);
                    i += 2;
                } else {
                    out.push(Tok::OpCompare);
                    i += 1;
                }
            }
            b'.' if i + 1 < bytes.len() && bytes[i + 1] == b'.' => {
                out.push(Tok::DotDot);
                i += 2;
            }
            b'$' => {
                i += 1;
                let start = i;
                while i < bytes.len() && is_ident_char(bytes[i]) {
                    i += 1;
                }
                if i == start {
                    return Err(ProjectParseError::UnexpectedChar('$', start - 1));
                }
                out.push(Tok::Var(source[start..i].to_string()));
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut buf = String::new();
                let mut closed = false;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c == b'"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if c == b'\\' && i + 1 < bytes.len() {
                        // Best-effort: keep escaped char as-is. We
                        // don't decode escapes since the value is
                        // ignored downstream.
                        buf.push(bytes[i + 1] as char);
                        i += 2;
                        continue;
                    }
                    buf.push(c as char);
                    i += 1;
                }
                if !closed {
                    return Err(ProjectParseError::UnterminatedString(start));
                }
                out.push(Tok::Str(buf));
            }
            _ if is_ident_start(b) => {
                let start = i;
                while i < bytes.len() && is_ident_char(bytes[i]) {
                    i += 1;
                }
                let ident = &source[start..i];
                // Look ahead for `.<ext>` to detect a file id; the
                // upstream lexer recognises `name.ext` as a single
                // FileId token.
                if i < bytes.len() && bytes[i] == b'.' {
                    let dot_at = i;
                    let mut j = i + 1;
                    while j < bytes.len() && is_ident_char(bytes[j]) {
                        j += 1;
                    }
                    if j > dot_at + 1 {
                        // Combine into a single FileId.
                        out.push(Tok::FileId(source[start..j].to_string()));
                        i = j;
                        continue;
                    }
                }
                if let Some(kw) = keyword(ident) {
                    out.push(kw);
                } else {
                    out.push(Tok::Id(ident.to_string()));
                }
            }
            _ if b.is_ascii_digit() => {
                // Numeric literal — not used by the MVP, just skip
                // through digits and treat as Id (a placeholder).
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                out.push(Tok::Id(source[start..i].to_string()));
            }
            _ => {
                return Err(ProjectParseError::UnexpectedChar(b as char, i));
            }
        }
    }
    Ok(out)
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_char(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

struct Parser<'a> {
    tokens: &'a [Tok],
    pos: usize,
    /// Output: file paths in encounter order.
    files: Vec<PathBuf>,
    /// Variable definitions: `variable NAME = VALUE`.
    variables: Vec<ProjectVariable>,
    /// Named module definitions.
    modules: Vec<ProjectModule>,
    /// Inter-module dependencies.
    dependencies: Vec<ProjectDependency>,
    /// Current module name (for tracking dependency `from` field).
    current_module: Option<String>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<&Tok> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_top_level(&mut self) -> Result<(), ProjectParseError> {
        while let Some(tok) = self.peek() {
            match tok {
                Tok::KwVariable => {
                    // `variable id = expr` — extract name and value.
                    self.bump(); // variable
                    self.parse_variable_decl();
                }
                Tok::KwTest => {
                    // `__test id1 id2 ...` — consume the keyword and
                    // skip the rest.
                    self.bump();
                    self.skip_until_top_level_break();
                }
                Tok::KwIf => {
                    // `if VAR then { ... } else { ... }` — conservative:
                    // collect files from both branches.
                    self.bump(); // if
                    self.skip_until_brace_or_then(); // skip condition
                                                     // Parse the then-branch (could be `then { ... }` or `{ ... }`)
                    if matches!(self.peek(), Some(Tok::KwThen)) {
                        self.bump(); // then
                    }
                    if matches!(self.peek(), Some(Tok::LCurly)) {
                        self.bump(); // {
                        self.parse_module_body(&PathBuf::new())?;
                    }
                    // Parse the else-branch if present
                    if matches!(self.peek(), Some(Tok::KwElse)) {
                        self.bump(); // else
                        if matches!(self.peek(), Some(Tok::LCurly)) {
                            self.bump(); // {
                            self.parse_module_body(&PathBuf::new())?;
                        }
                    }
                }
                Tok::Id(_) => {
                    // Module head: `<id> { body }`. We accept either
                    // `name {` directly or `name = expr` (which would
                    // be a stray top-level binding — skip).
                    self.parse_module_head_or_skip(&PathBuf::new())?;
                }
                Tok::Semi => {
                    self.bump();
                }
                _ => {
                    // Any other unexpected token at top level — skip
                    // it to stay resilient.
                    self.bump();
                }
            }
        }
        Ok(())
    }

    fn skip_until_brace_or_then(&mut self) {
        while let Some(tok) = self.peek() {
            match tok {
                Tok::LCurly | Tok::KwThen | Tok::KwElse => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn skip_until_top_level_break(&mut self) {
        // Skip tokens until we hit a top-level boundary: an explicit
        // `Semi`, the start of another keyword statement
        // (`KwVariable` / `KwTest`), or what looks like the head of
        // a module (`Id LCurly`). The goal is to recover from a
        // statement we don't model (e.g. `variable ARCH = A64`)
        // without swallowing the next module.
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Semi => {
                    self.bump();
                    return;
                }
                Tok::KwVariable | Tok::KwTest => return,
                Tok::Id(_) => {
                    if matches!(self.tokens.get(self.pos + 1), Some(Tok::LCurly)) {
                        return;
                    }
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Parse `<name> { body }` or skip if not actually a module.
    fn parse_module_head_or_skip(
        &mut self,
        directory_prefix: &Path,
    ) -> Result<(), ProjectParseError> {
        let _start = self.pos;
        let name = match self.bump() {
            Some(Tok::Id(s)) => s.clone(),
            _ => return Ok(()),
        };
        match self.peek() {
            Some(Tok::LCurly) => {
                self.bump();
                // Track current module for dependency extraction.
                let prev_module = self.current_module.take();
                self.current_module = Some(name.clone());

                let files_before = self.files.len();
                self.parse_module_body(directory_prefix)?;
                let module_files = self.files[files_before..].to_vec();

                // Record the module.
                self.modules.push(ProjectModule {
                    name: name.clone(),
                    files: module_files,
                    directory: None, // set during parse_module_body if present
                });

                self.current_module = prev_module;

                // Consume the matching `}`.
                if matches!(self.peek(), Some(Tok::RCurly)) {
                    self.bump();
                }
            }
            Some(Tok::Eq) => {
                // Stray top-level binding — skip its expression.
                self.bump();
                self.skip_expression();
            }
            _ => {
                // Bare identifier; skip and continue.
            }
        }
        Ok(())
    }

    fn parse_module_body(&mut self, outer_dir: &Path) -> Result<(), ProjectParseError> {
        // Track the current directory for this module scope. The
        // upstream grammar lets `directory <expr>` set a new prefix
        // applied to all subsequent `files` in the same scope.
        let mut current_dir = outer_dir.to_path_buf();
        let mut depth = 0_i32;

        while let Some(tok) = self.peek() {
            match tok {
                Tok::RCurly if depth == 0 => return Ok(()),
                Tok::LCurly => {
                    depth += 1;
                    self.bump();
                }
                Tok::RCurly => {
                    depth -= 1;
                    self.bump();
                }
                Tok::KwFiles => {
                    self.bump();
                    let collected = self.parse_expression_collecting_files();
                    for path in collected {
                        let mut full = current_dir.clone();
                        full.push(path);
                        self.files.push(full);
                    }
                }
                Tok::KwDirectory => {
                    self.bump();
                    let collected = self.parse_expression_collecting_strings();
                    if let Some(first) = collected.first() {
                        current_dir = outer_dir.join(first);
                    }
                }
                Tok::KwRequires | Tok::KwAfter | Tok::KwBefore => {
                    let kind = match tok {
                        Tok::KwRequires => DependencyKind::Requires,
                        Tok::KwAfter => DependencyKind::After,
                        Tok::KwBefore => DependencyKind::Before,
                        _ => unreachable!(),
                    };
                    self.bump();
                    // Collect dependency target names.
                    let targets = self.parse_expression_collecting_strings();
                    if let Some(from) = &self.current_module {
                        for to in targets {
                            self.dependencies.push(ProjectDependency {
                                kind,
                                from: from.clone(),
                                to,
                            });
                        }
                    }
                }
                Tok::KwVariable => {
                    self.bump();
                    self.parse_variable_decl();
                }
                Tok::Id(_) => {
                    // Could be a nested module head: `name { ... }`.
                    self.parse_module_head_or_skip(&current_dir)?;
                }
                Tok::Semi => {
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
        Ok(())
    }

    /// Walk an expression token-by-token until a top-level break
    /// (`Semi`, `RCurly` at depth 0, or another keyword that starts a
    /// new clause), collecting any FileId tokens encountered. Both
    /// branches of `if/then/else` are walked.
    fn parse_expression_collecting_files(&mut self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut depth: i32 = 0;
        while let Some(tok) = self.peek() {
            match tok {
                Tok::LSquare | Tok::LCurly | Tok::LParen => {
                    depth += 1;
                    self.bump();
                }
                Tok::RSquare | Tok::RParen => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.bump();
                }
                Tok::RCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.bump();
                }
                Tok::Semi if depth == 0 => break,
                Tok::KwFiles
                | Tok::KwDirectory
                | Tok::KwRequires
                | Tok::KwAfter
                | Tok::KwBefore
                    if depth == 0 =>
                {
                    break;
                }
                Tok::FileId(name) => {
                    out.push(PathBuf::from(name));
                    self.bump();
                }
                Tok::Str(s) if s.contains('.') => {
                    // Quoted file path is also acceptable.
                    out.push(PathBuf::from(s));
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
        out
    }

    /// Like `parse_expression_collecting_files` but collects bare
    /// identifiers/strings (used by `directory <expr>` clauses).
    fn parse_expression_collecting_strings(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let mut depth: i32 = 0;
        while let Some(tok) = self.peek() {
            match tok {
                Tok::LSquare | Tok::LCurly | Tok::LParen => {
                    depth += 1;
                    self.bump();
                }
                Tok::RSquare | Tok::RParen => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.bump();
                }
                Tok::RCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.bump();
                }
                Tok::Semi if depth == 0 => break,
                Tok::KwFiles
                | Tok::KwDirectory
                | Tok::KwRequires
                | Tok::KwAfter
                | Tok::KwBefore
                    if depth == 0 =>
                {
                    break;
                }
                Tok::Id(s) => {
                    out.push(s.clone());
                    self.bump();
                }
                Tok::Str(s) => {
                    out.push(s.clone());
                    self.bump();
                }
                Tok::FileId(s) => {
                    out.push(s.clone());
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
        out
    }

    /// Parse `variable NAME = VALUE`.
    fn parse_variable_decl(&mut self) {
        // Expect: Id Eq (value)
        let name = match self.peek() {
            Some(Tok::Id(s)) => {
                let s = s.clone();
                self.bump();
                s
            }
            Some(Tok::Var(s)) => {
                let s = s.clone();
                self.bump();
                s
            }
            _ => {
                self.skip_until_top_level_break();
                return;
            }
        };

        // Skip `=`
        if matches!(self.peek(), Some(Tok::Eq)) {
            self.bump();
        }

        // Collect the value — a single identifier, string, or boolean.
        let value = match self.peek() {
            Some(Tok::Id(s)) | Some(Tok::Str(s)) | Some(Tok::FileId(s)) => {
                let v = s.clone();
                self.bump();
                v
            }
            Some(Tok::KwTrue) => {
                self.bump();
                "true".to_string()
            }
            Some(Tok::KwFalse) => {
                self.bump();
                "false".to_string()
            }
            _ => String::new(),
        };

        // Skip any remaining tokens in the variable statement.
        self.skip_until_top_level_break();

        self.variables.push(ProjectVariable { name, value });
    }

    fn skip_expression(&mut self) {
        let _ = self.parse_expression_collecting_files();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &ProjectFile) -> Vec<String> {
        p.files.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn parses_simple_two_module_project() {
        let src = "\
A {
  files amod.sail
}

B {
  requires A
  files bmod.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["amod.sail", "bmod.sail"]);
    }

    #[test]
    fn parses_nested_files_with_subdirs() {
        let src = "\
A {
  files simple/amod.sail
}

B {
  requires A
  files simple/bmod.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["simple/amod.sail", "simple/bmod.sail"]);
    }

    #[test]
    fn parses_files_list_with_commas_and_brackets() {
        let src = "\
M {
  files
    a.sail,
    b.sail,
    [
      c.sail,
      d.sail
    ]
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["a.sail", "b.sail", "c.sail", "d.sail"]);
    }

    #[test]
    fn collects_both_branches_of_conditional() {
        // Conservative: take both branches of an if/else so the LSP
        // indexes a superset of files rather than a subset.
        let src = "\
M {
  files
    if $ARCH == A32 then arch_xlen32.sail
    else [
      arch_xlen64.sail,
      arch_xlen64_helpers.sail
    ]
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(
            names(&parsed),
            vec!["arch_xlen32.sail", "arch_xlen64.sail", "arch_xlen64_helpers.sail"]
        );
    }

    #[test]
    fn dedupes_repeated_files_preserving_first_occurrence() {
        let src = "\
A {
  files shared.sail
}

B {
  files shared.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["shared.sail"]);
    }

    #[test]
    fn directory_clause_prefixes_files() {
        let src = "\
M {
  directory subdir
  files inner.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["subdir/inner.sail"]);
    }

    #[test]
    fn variable_and_test_decls_are_skipped() {
        let src = "\
variable ARCH = A64
__test foo bar

M {
  files main.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["main.sail"]);
    }

    #[test]
    fn line_comments_are_ignored() {
        let src = "\
// top comment
M {
  // before files
  files a.sail // trailing
  // after files
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["a.sail"]);
    }

    #[test]
    fn block_comments_are_ignored() {
        let src = "\
/* leading */ M {
  files /* inline */ a.sail /* nested /* hi */ */
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["a.sail"]);
    }

    #[test]
    fn requires_clause_does_not_inject_phantom_files() {
        // The argument of `requires` is a list of module names, not
        // files; it should not contribute to the file list.
        let src = "\
A {
  files real.sail
}

B {
  requires A
  files other.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(names(&parsed), vec!["real.sail", "other.sail"]);
    }

    #[test]
    fn empty_project_returns_empty_list() {
        let parsed = parse_project("").expect("parse");
        assert_eq!(names(&parsed), Vec::<String>::new());
    }

    #[test]
    fn nested_modules_inherit_directory_prefix_from_outer() {
        // Note: the inner module's directory does NOT inherit the
        // outer module's `directory` clause in the upstream grammar
        // either — `directory` is a per-scope statement. Verifying
        // current behavior matches that.
        let src = "\
Outer {
  directory subdir
  files outer.sail
  Inner {
    files inner.sail
  }
}
";
        let parsed = parse_project(src).expect("parse");
        // Outer's directory applies to its own files clause; Inner
        // gets the outer's directory passed in as its own scope's
        // base, since we thread current_dir into nested module heads.
        assert_eq!(names(&parsed), vec!["subdir/outer.sail", "subdir/inner.sail"]);
    }

    #[test]
    fn parses_sail_riscv_project_file() {
        // Test against the real sail-riscv project file structure.
        // Uses a subset of the actual riscv.sail_project format.
        let src = r#"
variable RMEM = false

prelude {
  files
    prelude/prelude.sail,
    prelude/errors.sail,
}

core {
  requires prelude

  files
    core/xlen.sail,
    core/flen.sail,
    core/types.sail,
}

rv32 {
  requires core

  directory rv32
  files
    rv32i.sail,
    rv32m.sail,
}

rv64 {
  requires core

  directory rv64
  files
    rv64i.sail,
    rv64m.sail,
}

postlude {
  requires core

  files
    postlude/step.sail,
    postlude/insts_end.sail,
}
"#;
        let parsed = parse_project(src).expect("should parse sail-riscv-like project");
        let file_names = names(&parsed);

        // Prelude files
        assert!(
            file_names.contains(&"prelude/prelude.sail".to_string()),
            "should contain prelude: {:?}",
            file_names
        );
        assert!(file_names.contains(&"prelude/errors.sail".to_string()));

        // Core files
        assert!(file_names.contains(&"core/xlen.sail".to_string()));
        assert!(file_names.contains(&"core/types.sail".to_string()));

        // RV32 files (with directory prefix)
        assert!(
            file_names.contains(&"rv32/rv32i.sail".to_string()),
            "rv32 directory prefix should apply: {:?}",
            file_names
        );
        assert!(file_names.contains(&"rv32/rv32m.sail".to_string()));

        // RV64 files
        assert!(file_names.contains(&"rv64/rv64i.sail".to_string()));

        // Postlude
        assert!(file_names.contains(&"postlude/step.sail".to_string()));
        assert!(file_names.contains(&"postlude/insts_end.sail".to_string()));

        // Total count should be reasonable
        assert!(file_names.len() >= 9, "expected at least 9 files, got {}", file_names.len());
    }

    #[test]
    #[ignore] // Requires sail-riscv on disk
    #[allow(clippy::print_stderr)]
    fn parses_actual_sail_riscv_project_file() {
        let path = "/home/clair/tinyueng_workplace/sail-riscv/model/riscv.sail_project";
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("Skipping: {path} not found");
            return;
        };
        let parsed = parse_project(&src).expect("should parse actual sail-riscv project");
        let file_names = names(&parsed);
        eprintln!("Parsed {} files from sail-riscv project", file_names.len());
        assert!(
            file_names.len() >= 50,
            "sail-riscv should have 50+ files, got {}",
            file_names.len()
        );
        // Spot check a few known files
        assert!(
            file_names.iter().any(|f| f.contains("prelude.sail")),
            "should contain prelude: {:?}",
            &file_names[..5]
        );
    }

    #[test]
    fn handles_if_then_else_in_project() {
        // sail-riscv uses `if RMEM then { ... } else { ... }` in project files
        let src = r#"
prelude {
  files prelude.sail
}
if RMEM then {
  files rmem.sail
} else {
  files no_rmem.sail
}
"#;
        let parsed = parse_project(src).expect("should parse if/then/else");
        let file_names = names(&parsed);
        // Conservative: both branches collected
        assert!(file_names.contains(&"prelude.sail".to_string()));
        // At least one of the conditional files should be present
        assert!(file_names.len() >= 2, "should collect conditional files: {:?}", file_names);
    }

    #[test]
    fn extracts_variable_definitions() {
        let src = "\
variable ARCH = A64
variable RMEM = false

M {
  files main.sail
}
";
        let parsed = parse_project(src).expect("parse");
        assert_eq!(parsed.variables.len(), 2);
        assert_eq!(parsed.variables[0].name, "ARCH");
        assert_eq!(parsed.variables[0].value, "A64");
        assert_eq!(parsed.variables[1].name, "RMEM");
        assert_eq!(parsed.variables[1].value, "false");
    }

    #[test]
    fn extracts_module_names() {
        let src = "\
prelude {
  files prelude.sail
}

core {
  requires prelude
  files core.sail
}
";
        let parsed = parse_project(src).expect("parse");
        let module_names: Vec<_> = parsed.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(module_names.contains(&"prelude"), "modules: {module_names:?}");
        assert!(module_names.contains(&"core"), "modules: {module_names:?}");
    }

    #[test]
    fn extracts_dependencies() {
        let src = "\
A {
  files a.sail
}

B {
  requires A
  after A
  files b.sail
}

C {
  requires A
  requires B
  before A
  files c.sail
}
";
        let parsed = parse_project(src).expect("parse");
        let deps: Vec<_> = parsed
            .dependencies
            .iter()
            .map(|d| (format!("{:?}", d.kind), d.from.as_str(), d.to.as_str()))
            .collect();
        assert!(deps.contains(&("Requires".to_string(), "B", "A")), "deps: {deps:?}");
        assert!(deps.contains(&("After".to_string(), "B", "A")), "deps: {deps:?}");
        assert!(deps.contains(&("Requires".to_string(), "C", "A")), "deps: {deps:?}");
        assert!(deps.contains(&("Requires".to_string(), "C", "B")), "deps: {deps:?}");
        assert!(deps.contains(&("Before".to_string(), "C", "A")), "deps: {deps:?}");
    }

    #[test]
    fn module_files_are_tracked() {
        let src = "\
core {
  directory src
  files core.sail
}
ext {
  files ext.sail
}
";
        let parsed = parse_project(src).expect("parse");
        let core_mod = parsed.modules.iter().find(|m| m.name == "core");
        assert!(core_mod.is_some(), "core module should exist");
        let core_files: Vec<_> =
            core_mod.unwrap().files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        assert!(core_files.iter().any(|f| f.contains("core.sail")), "core files: {core_files:?}");

        let ext_mod = parsed.modules.iter().find(|m| m.name == "ext");
        assert!(ext_mod.is_some(), "ext module should exist");
    }

    #[test]
    fn variable_with_string_value() {
        let src = r#"
variable ARCH = "riscv64"
M {
  files main.sail
}
"#;
        let parsed = parse_project(src).expect("parse");
        assert_eq!(parsed.variables.len(), 1);
        assert_eq!(parsed.variables[0].name, "ARCH");
        assert_eq!(parsed.variables[0].value, "riscv64");
    }
}
