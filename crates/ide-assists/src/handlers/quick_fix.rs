//! Quick fixes: the assists a *diagnostic* offers, rather than the ones a
//! cursor position offers.
//!
//! They lived in the crate root, which is where rust-analyzer keeps three
//! items and no handler — every assist of theirs is under `handlers/`, which
//! is also where `all()` looks. A fix in the root is one `all()` cannot
//! register, so it reaches an editor only if some other code path calls it
//! by name; `quick_fix_for_diagnostic` is that path, and it is hand-written.

use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::FileDb;
use ide_diagnostics::Diagnostic;

pub fn missing_semicolon_fix(file: &dyn FileDb, diagnostic: &Diagnostic) -> Option<IdeTextEdit> {
    let message = diagnostic.message.to_ascii_lowercase();
    if !message.contains("expected") || !message.contains(';') {
        return None;
    }

    let line_index = ide_db::line_index::LineIndex::new(file.text());
    let start_lc = line_index.line_col(base_db::range_start(diagnostic.range.range));
    let line = start_lc.line;
    let text = line_text(file, line)?;
    let mut logical = text.trim_end_matches(['\n', '\r']);
    if let Some((head, _)) = logical.split_once("//") {
        logical = head;
    }
    let logical = logical.trim_end();
    if logical.is_empty() || logical.ends_with(';') {
        return None;
    }
    if logical.ends_with('{') || logical.ends_with('}') {
        return None;
    }

    let base = file.offset_at(&ide_db::LineCol { line, col: 0 });
    let insert_offset = base + logical.len();

    Some(IdeTextEdit {
        range: base_db::text_range(insert_offset, insert_offset),
        new_text: ";".to_string(),
    })
}

/// Quick fix for a diagnostic — returns internal IdeTextEdit.
pub fn quick_fix_for_diagnostic(
    file: &dyn FileDb,
    diagnostic: &Diagnostic,
) -> Option<(String, IdeTextEdit, bool)> {
    if let Some(fix) = unused_variable_fix(file, diagnostic) {
        return Some(fix);
    }
    // C2: Quick-fix for incomplete match — add wildcard arm.
    if diagnostic.code.as_str() == "incomplete-match" {
        return incomplete_match_fix(file, diagnostic);
    }

    let token = expected_token_from_message(&diagnostic.message)?;
    if token == ";" {
        let edit = missing_semicolon_fix(file, diagnostic)?;
        return Some(("Insert missing `;`".to_string(), edit, true));
    }
    let edit = missing_token_fix(file, diagnostic, token)?;
    Some((format!("Insert missing `{token}`"), edit, false))
}

/// Quick fix for `unused-variable`: prefix the variable name with `_`
/// to silence the warning while preserving the binding's structural role.
/// This matches both the upstream Sail convention (`_` prefix marks
/// intentionally-unused identifiers) and rust-analyzer's suggestion.
pub fn unused_variable_fix(
    file: &dyn FileDb,
    diagnostic: &Diagnostic,
) -> Option<(String, IdeTextEdit, bool)> {
    if diagnostic.code.as_str() != "unused-variable" {
        return None;
    }
    // The diagnostic range is the variable name. Extract it from the source
    // to make sure we're not double-prefixing an already-`_`-prefixed name.
    let start_offset = base_db::range_start(diagnostic.range.range);
    let end_offset = base_db::range_end(diagnostic.range.range);
    let name = file.text().get(start_offset..end_offset)?;
    if name.starts_with('_') {
        return None;
    }
    Some((
        format!("Rename `{name}` to `_{name}`"),
        IdeTextEdit {
            range: base_db::text_range(start_offset, start_offset),
            new_text: "_".to_string(),
        },
        true,
    ))
}

pub fn var_to_let_fix(
    file: &dyn FileDb,
    diagnostic: &Diagnostic,
) -> Option<(String, IdeTextEdit, bool)> {
    if diagnostic.code.as_str() != "unmodified-mutable-variable" {
        return None;
    }
    // The diagnostic range points to the variable name. We need to find the `var` keyword before it.
    let name_start = base_db::range_start(diagnostic.range.range);
    let prefix = &file.text()[..name_start];
    let var_start = prefix.rfind("var")?;
    // Verify it's the keyword (preceded by whitespace or line start)
    if var_start > 0 {
        let before = file.text().as_bytes()[var_start - 1];
        if before != b' '
            && before != b'\t'
            && before != b'\n'
            && before != b'\r'
            && before != b'{'
            && before != b';'
        {
            return None;
        }
    }
    let var_end = var_start + 3; // "var".len()
    Some((
        "Change `var` to `let`".to_string(),
        IdeTextEdit { range: base_db::text_range(var_start, var_end), new_text: "let".to_string() },
        true,
    ))
}

#[cfg(test)]
mod match_fix_tests {
    use super::*;
    use hir::diagnostics::DiagnosticCode;
    use ide_db::test_utils::TestFile;

    #[test]
    fn wildcard_fix_uses_the_match_token_and_preserves_result_type() {
        for arm in ["true => 7", "true => 7, /* } */", "true => if false then 7 else 8"] {
            let source = format!("val f : bool -> int\nfunction f(x) = match x {{ {arm} }}\n");
            let file = TestFile::new(&source);
            let root = file.syntax().unwrap();
            let node =
                root.descendants().find(|n| n.kind() == syntax::SyntaxKind::MATCH_EXPR).unwrap();
            let diagnostic = Diagnostic::new(
                DiagnosticCode::SailError("incomplete-match"),
                "incomplete match",
                hir::FileRange { file_id: base_db::FileId::from_raw(0), range: node.text_range() },
            );
            let (_, edit, preferred) = quick_fix_for_diagnostic(&file, &diagnostic).unwrap();
            assert!(!preferred);
            assert_eq!(usize::from(edit.range.start()), source.rfind('}').unwrap());
            let mut result = source.clone();
            result.insert_str(usize::from(edit.range.start()), &edit.new_text);
            assert!(result.contains("_ => undefined"));
            assert!(syntax::parse_text(&result).1.is_empty(), "{result}");
        }
    }
}
