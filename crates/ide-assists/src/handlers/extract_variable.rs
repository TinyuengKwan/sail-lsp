//! `extract_variable` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn extract_variable(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = extract_local_let_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("extract_variable", AssistKind::RefactorExtract),
        "Extract to variable",
        ctx.range,
        te,
    );
    Some(())
}

pub fn extract_local_let_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    use parser::SyntaxKind as SK;

    let start = base_db::range_start(range);
    let end = base_db::range_end(range);
    if start >= end {
        return None;
    }

    let text = file.text();
    let selected = text.get(start..end)?.trim();
    if selected.is_empty() || selected.contains('\n') || selected.contains('\r') {
        return None;
    }

    let binding = "extracted_value";
    // The file's own tree where it has one. `FileDb::syntax()` defaults
    // to `None`, so a handle that did not override it still needs the
    // text parsed.
    let root = file.syntax().unwrap_or_else(|| syntax::parse_text(text).0);

    // The block the selection sits in, if any. Inserting at the start of
    // the selection's *line* — which is what this did — puts the binding
    // outside the function whenever the body is written on one line:
    // `function f(x) = x + 1` became a top-level `let` naming a parameter
    // that is not in scope there.
    let covering = root
        .descendants()
        .filter(|n| {
            usize::from(n.text_range().start()) <= start && end <= usize::from(n.text_range().end())
        })
        .last()?;

    if covering.ancestors().any(|n| n.kind() == SK::BLOCK_EXPR) {
        // Inside a block, the binding goes on its own line above the one
        // the selection is on, at that line's indent. Strictly before the
        // selection, so the two edits cannot land on the same offset and
        // clobber each other.
        let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent: String =
            text[line_start..start].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        return Some(vec![
            IdeTextEdit {
                range: base_db::text_range(line_start, line_start),
                new_text: format!("{indent}let {binding} = {selected};\n"),
            },
            IdeTextEdit { range, new_text: binding.to_owned() },
        ]);
    }

    // No block: the body is a bare expression, so extracting means making
    // one. `function f(x) = x + 1` becomes `function f(x) = { let v = x + 1; v }`.
    let body = covering
        .ancestors()
        .take_while(|n| n.kind() != SK::CALLABLE_DEF)
        .last()
        .filter(|n| n.kind() != SK::CALLABLE_DEF)?;
    let body_start = usize::from(body.text_range().start());
    let body_end = usize::from(body.text_range().end());
    if start < body_start || end > body_end {
        return None;
    }
    let mut rewritten = text.get(body_start..body_end)?.to_owned();
    rewritten.replace_range(start - body_start..end - body_start, binding);
    Some(vec![IdeTextEdit {
        range: base_db::text_range(body_start, body_end),
        new_text: format!("{{ let {binding} = {selected}; {rewritten} }}"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    /// A body that is a bare expression has no block to put the binding
    /// in, so extracting makes one. This used to insert at the start of the
    /// selection's *line*, which for a one-line body is column zero: the
    /// `let` landed at the top level, naming a parameter that is not in
    /// scope there.
    #[test]
    fn applies() {
        check_assist(
            extract_variable,
            "val f : int -> int
function f(x) = $0x + 1$0
",
            expect![[r#"
                val f : int -> int
                function f(x) = { let extracted_value = x + 1; extracted_value }
            "#]],
        );
    }

    /// With a block already there, the binding goes before the statement.
    #[test]
    fn extracts_inside_an_existing_block() {
        check_assist(
            extract_variable,
            "val f : int -> int\nfunction f(x) = {\n  let y = 1;\n  $0x + y$0\n}\n",
            expect![[r#"
                val f : int -> int
                function f(x) = {
                  let y = 1;
                  let extracted_value = x + y;
                  extracted_value
                }
            "#]],
        );
    }
}
