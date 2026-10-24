//! Rename.
//!
//! `Analysis::rename` is the whole feature; this module holds the one piece
//! the server asks for separately — whether the token under the cursor can
//! be renamed at all, and what its current text is.
//!
//! There used to be a second rename here, reaching `Definition::rename` in
//! ide-db through a `definition_from_decl` that returned `None`
//! unconditionally, plus a `rename_legacy` kept "until the handlers
//! migrate". Nothing called either. What the handlers actually call is
//! `Analysis::rename`, which goes through `find_all_refs`.

use ide_db::line_index::{LineCol, TextRange};
use ide_db::FileDb;

// Re-export from ide_db::rename for downstream consumers.
pub use ide_db::rename::{IdentifierKind as IdKind, RenameError as Error};

pub use crate::navigation::will_rename_file_edits as will_rename_file;
pub use crate::references::{normalize_validated_rename, rename_edits};

/// The name under the cursor and its range, if it can be renamed.
///
/// Pragmas (`$include`) and attributes (`@name`) are written with a sigil
/// and are not names the workspace declares, so neither is renameable.
pub fn prepare_rename(file: &dyn FileDb, position: LineCol) -> Option<(String, TextRange)> {
    let (token, span) = file.token_at(position)?;
    let name = ide_db::token_symbol_key(token)?;

    if name.is_empty() || name.starts_with('$') || name.starts_with('@') {
        return None;
    }

    Some((name, base_db::text_range(span.start, span.end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::rename::IdentifierKind;
    use ide_db::test_utils::TestFile;

    #[test]
    fn prepare_rename_on_identifier() {
        let source = "function foo(x) = x\n";
        let file = TestFile::new(source);
        let pos = file.position_at(9);
        let result = prepare_rename(&file, pos);
        assert!(result.is_some(), "should be able to rename 'foo'");
        let (name, _range) = result.unwrap();
        assert_eq!(name, "foo");
    }

    #[test]
    fn classify_rejects_keywords() {
        let result = IdentifierKind::classify("function");
        assert!(result.is_err(), "keyword should be rejected");
    }

    #[test]
    fn classify_rejects_empty() {
        let result = IdentifierKind::classify("");
        assert!(result.is_err(), "empty name should be rejected");
    }

    #[test]
    fn classify_accepts_valid_ident() {
        let result = IdentifierKind::classify("new_foo");
        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod through_analysis {
    use crate::fixture::check_rename;

    /// A scattered definition's head, its clauses and its `end` all name the
    /// same function; renaming has to reach every one.
    #[test]
    fn rename_covers_a_scattered_definition() {
        check_rename(
            r#"
//- /main.sail
scattered function exec$0ute
function clause execute(x) = x
end execute
"#,
            "run",
            3,
        );
    }
}

#[cfg(test)]
mod through_analysis_more {
    use crate::fixture::check_rename;

    #[test]
    fn rename_a_plain_function_and_its_call() {
        check_rename(
            r#"
//- /main.sail
function ad$0d(x : int, y : int) -> int = x + y
function main() -> int = add(1, 2)
"#,
            "plus",
            2,
        );
    }
}
