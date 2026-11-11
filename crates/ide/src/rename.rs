//! Rename.
//!
//! `Analysis::rename` is the whole feature; this module holds the one piece
//! the server asks for separately — whether the token under the cursor can
//! be renamed at all, and what its current text is.
//!
use ide_db::line_index::{LineCol, TextRange};
use ide_db::rename::RenameError;
use ide_db::FileDb;

/// The name under the cursor and its range, or why it cannot be renamed.
///
/// Every refusal has a reason, as rust-analyzer's does, because the server
/// sends it: a `null` here showed the editor's generic "cannot be renamed",
/// which says nothing about a keyword, a pragma or an empty line.
///
/// Pragmas (`$include`) and attributes (`@name`) are written with a sigil
/// and are not names the workspace declares, so neither is renameable.
pub fn prepare_rename(
    file: &dyn FileDb,
    position: LineCol,
) -> Result<(String, TextRange), RenameError> {
    let Some((token, span)) = file.token_at(position) else {
        return Err(RenameError("No references found at position".into()));
    };
    let Some(name) = ide_db::token_symbol_key(token).filter(|name| !name.is_empty()) else {
        // Not a name. The token's own text says which kind of not-a-name.
        let text = token.to_string();
        let reason = if ide_db::keywords::is_keyword(&text) {
            "Cannot rename a keyword"
        } else if text.starts_with('$') {
            "Cannot rename a pragma"
        } else if text.starts_with('@') {
            "Cannot rename an attribute"
        } else {
            "No references found at position"
        };
        return Err(RenameError(reason.into()));
    };

    Ok((name, base_db::text_range(span.start, span.end)))
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
        assert!(result.is_ok(), "should be able to rename 'foo'");
        let (name, _range) = result.unwrap();
        assert_eq!(name, "foo");
    }

    /// Each refusal names its reason: the server sends the message, and a
    /// `None` here reached the editor as a generic "cannot be renamed".
    #[test]
    fn prepare_rename_refuses_with_a_reason() {
        let source = "$include \"lib.sail\"\nfunction foo(x) = x\n\n";
        let file = TestFile::new(source);
        let reason = |offset: usize| prepare_rename(&file, file.position_at(offset)).unwrap_err().0;
        assert_eq!(reason(source.find("function").unwrap()), "Cannot rename a keyword");
        assert_eq!(reason(1), "Cannot rename a pragma");
        assert_eq!(reason(source.len() - 1), "No references found at position");
        assert_eq!(reason(source.find('=').unwrap()), "No references found at position");
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
