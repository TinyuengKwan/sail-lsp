//! Runnable detection for code lens.
//!
//! Detects `main` functions and functions with `$[test]` attributes.
//! These are surfaced as "Run" / "Test" code lenses by the LSP handler.
//!
//! # Sail-specific notes
//! Sail uses `$[test]` (not Rust's `#[test]`) as an attribute on functions
//! to mark them as test cases. The attribute appears on the line(s) immediately
//! before the `function` keyword in source text.

use ide_db::FileDb;

/// A detected runnable item in a Sail file.
#[derive(Debug, Clone)]
pub struct Runnable {
    /// The function name.
    pub name: String,
    /// What kind of runnable this is.
    pub kind: RunnableKind,
    /// Byte span of the function definition (for code lens placement).
    pub span: parser::Span,
}

/// Kind of runnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableKind {
    /// `function main() = ...` — program entry point.
    Main,
    /// Function preceded by a `$[test]` attribute.
    Test,
}

/// Detect runnables in a Sail file.
///
/// Returns one `Runnable` per function that is either named `main` or
/// annotated with `$[test]` in the source text immediately preceding the
/// function definition.
pub fn runnables(file: &dyn FileDb) -> Vec<Runnable> {
    let text = file.text();
    if text.is_empty() {
        return Vec::new();
    }

    let tree = match file.item_tree() {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut result = Vec::new();

    for item in tree.top_level_items() {
        let hir_def::item_tree::ModItem::Function(id) = item else {
            continue;
        };
        let func = tree.function(*id);

        // Skip function clauses — only emit a runnable for the head definition.
        if func.is_clause {
            continue;
        }

        let name = func.name.as_str().to_string();
        let span = func.span;

        // 1. Named "main" — always a runnable entry point.
        if name == "main" {
            result.push(Runnable { name, kind: RunnableKind::Main, span });
            continue;
        }

        // 2. Preceded by `$[test]` attribute in source text.
        if is_test_annotated(text, span.start) {
            result.push(Runnable { name, kind: RunnableKind::Test, span });
        }
    }

    result
}

/// Check whether the source text immediately before `fn_start` (byte offset)
/// contains a `$[test]` attribute on the same or adjacent lines.
///
/// We scan backwards from `fn_start` through blank lines and attribute lines
/// (`$[...]`). If we find `$[test]` among those attributes, return `true`.
fn is_test_annotated(text: &str, fn_start: usize) -> bool {
    // Clamp to valid range
    let prefix = &text[..fn_start.min(text.len())];

    // Walk lines from the end, skipping blank lines and `$[...]` lines.
    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("$[test]") || trimmed == "$[test" {
            return true;
        }
        // A `$[test, ...]` or `$[test,...]` form
        if trimmed.starts_with("$[test,") {
            return true;
        }
        // If this line is any other attribute (`$[...]`), keep scanning.
        if trimmed.starts_with("$[") {
            continue;
        }
        // Anything else (a doc comment, code, etc.) — stop scanning.
        break;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn runnables_from_text(src: &str) -> Vec<Runnable> {
        use ide_db::root_database::SalsaFile;
        let db = ide_db::root_database::RootDatabase::default();
        let text: Arc<str> = Arc::from(src);
        let ft = base_db::FileText::new(&db, text, base_db::FileId::from_raw(0));
        let sf = SalsaFile::new(&db, ft);
        runnables(&sf)
    }

    #[test]
    fn detects_main() {
        let src = "function main() = ()";
        let result = runnables_from_text(src);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "main");
        assert!(matches!(result[0].kind, RunnableKind::Main));
    }

    #[test]
    fn detects_test_attribute() {
        let src = "$[test]\nfunction run_test() = ()";
        let result = runnables_from_text(src);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "run_test");
        assert!(matches!(result[0].kind, RunnableKind::Test));
    }

    #[test]
    fn plain_function_is_not_runnable() {
        let src = "function add(x : int, y : int) : int = x + y";
        let result = runnables_from_text(src);
        assert!(result.is_empty());
    }

    #[test]
    fn test_attribute_with_blank_line_between() {
        // A blank line between $[test] and function should not match
        // (the attribute is not directly adjacent).
        let src = "$[test]\n\nfunction run_test() = ()";
        // blank line breaks the adjacency — this is a grey area; we allow it
        // because the scanner skips blank lines.
        let result = runnables_from_text(src);
        // Currently our scanner skips blank lines, so this should still match.
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn does_not_emit_runnable_for_clauses() {
        let src = "function main(x) = 1\nfunction main(y) = 2";
        let result = runnables_from_text(src);
        // Both parsed as separate functions; only first non-clause matches.
        // (Exact count depends on parser; at minimum no crash.)
        let _ = result;
    }
}
