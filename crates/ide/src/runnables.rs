//! What can be run in a file — rust-analyzer's `ide/src/runnables.rs`.
//!
//! Upstream Sail has no test runner. `$[test]` marks a `unit -> unit`
//! function (`type_check.ml`'s `check_test_attribute`), and the only
//! consumer in `sail` 0.20.2 is the C backend, which emits the
//! `SAIL_TESTS`/`SAIL_TEST_NAMES` arrays for a host harness to iterate.
//! The one way the compiler itself executes such a function is its
//! interpreter: `sail <project args> --is <script>`, where the script
//! evaluates `name()` and `:run`s it to completion. That is the command the
//! server attaches to each entry here; this module only finds them.
//!
//! The `main` entry follows the same convention the C backend uses when it
//! generates `main()`: it calls the Sail function named `main`.

use crate::navigation_target::NavigationTarget;
use ide_db::ide_types::SymbolKind;
use ide_db::FileDb;
use parser::SyntaxKind as SK;

#[derive(Debug, Clone)]
pub struct Runnable {
    pub nav: NavigationTarget,
    pub kind: RunnableKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableKind {
    /// A `$[test]` function; `nav.name` is what the interpreter calls.
    Test,
    /// The `main` function.
    Main,
}

impl Runnable {
    /// The interpreter script that runs this entry to completion.
    pub fn interpreter_script(&self) -> String {
        format!("{}()\n:run\n:quit\n", self.nav.name)
    }

    pub fn label(&self) -> String {
        match self.kind {
            RunnableKind::Test => format!("Run test {}", self.nav.name),
            RunnableKind::Main => "Run main".to_owned(),
        }
    }
}

/// Every `$[test]` function and the `main` function of a file, in source
/// order. A `$[test]` on a `val` and one on its `function` name the same
/// test; the first occurrence wins.
pub fn runnables(file: &dyn FileDb) -> Vec<Runnable> {
    let Some(root) = file.syntax() else {
        return Vec::new();
    };
    let placeholder = url::Url::parse("file:///").expect("a literal URL parses");
    let mut out: Vec<Runnable> = Vec::new();
    for node in root.children() {
        if !matches!(node.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
            continue;
        }
        let Some(name) = syntax::definition_name(&node) else { continue };
        let kind = if has_test_attribute(&node) {
            RunnableKind::Test
        } else if name == "main" && node.kind() == SK::CALLABLE_DEF {
            RunnableKind::Main
        } else {
            continue;
        };
        if out.iter().any(|r| r.nav.name == name) {
            continue;
        }
        let full = node.text_range();
        let focus = syntax::definition_name_span(&node)
            .map(|span| base_db::text_range(span.start, span.end))
            .unwrap_or(full);
        out.push(Runnable {
            nav: NavigationTarget::new(
                placeholder.clone(),
                name,
                SymbolKind::Function,
                full,
                focus,
            ),
            kind,
        });
    }
    out
}

/// Attributes are the declaration's preceding siblings in the lossless tree.
fn has_test_attribute(node: &syntax::SyntaxNode) -> bool {
    let mut previous = node.prev_sibling();
    while let Some(attr) = previous.filter(|n| n.kind() == SK::ATTRIBUTE) {
        let name = attr
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
            .map(|t| t.text().to_owned());
        if name.as_deref() == Some("test") {
            return true;
        }
        previous = attr.prev_sibling();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn test_functions_and_main_are_runnable_in_source_order() {
        let text = "val helper : int -> int\n\
                    function helper(x) = x\n\
                    $[test]\n\
                    function test_first() -> unit = assert(true)\n\
                    $[test]\n\
                    val test_second : unit -> unit\n\
                    function test_second() = ()\n\
                    function main() -> unit = test_first()\n";
        let file = TestFile::new(text);
        let found = runnables(&file);
        let summary: Vec<_> =
            found.iter().map(|r| (r.nav.name.as_str(), r.kind, r.label())).collect();
        assert_eq!(
            summary,
            [
                ("test_first", RunnableKind::Test, "Run test test_first".to_owned()),
                ("test_second", RunnableKind::Test, "Run test test_second".to_owned()),
                ("main", RunnableKind::Main, "Run main".to_owned()),
            ]
        );
        assert_eq!(found[0].interpreter_script(), "test_first()\n:run\n:quit\n");
        // The entry sits on the function, not on its attribute, and the
        // focus is the name.
        let first = &found[0];
        assert_eq!(&text[first.focus_text_range()], "test_first");
        assert!(text[first.full_text_range()].starts_with("function test_first"));
        // The `val` carries the attribute and is where the entry points; the
        // `function` below it is the same test, not a second one.
        let second = &found[1];
        assert!(text[second.full_text_range()].starts_with("val test_second"));
    }

    #[test]
    fn plain_functions_and_other_attributes_are_not_runnable() {
        let file = TestFile::new(
            "$[complete]\nfunction f() -> unit = ()\nval main : unit -> unit\nfunction g() -> unit = ()\n",
        );
        assert!(runnables(&file).is_empty());
    }

    impl Runnable {
        fn focus_text_range(&self) -> std::ops::Range<usize> {
            usize::from(self.nav.focus_range.start())..usize::from(self.nav.focus_range.end())
        }
        fn full_text_range(&self) -> std::ops::Range<usize> {
            usize::from(self.nav.full_range.start())..usize::from(self.nav.full_range.end())
        }
    }
}
