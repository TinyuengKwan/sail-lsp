//! `invert_if` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn invert_if(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = invert_if_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("invert_if", AssistKind::RefactorRewrite),
        "Invert if",
        ctx.range,
        te,
    );
    Some(())
}

/// Given a cursor position inside an `if` expression, produce edits that invert the condition
/// and swap the then/else branches.
pub fn invert_if_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let (if_id, expr_span) = find_if_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let hir::Expr::If { cond, then_branch, else_branch: Some(else_branch) } =
        entry.body.expr(if_id)?
    else {
        return None;
    };

    let text = file.text();
    let cond_span = entry.source_map.expr_syntax(*cond)?;
    let then_span = entry.source_map.expr_syntax(*then_branch)?;
    let else_span = entry.source_map.expr_syntax(*else_branch)?;
    let cond_text = text.get(cond_span.start..cond_span.end)?;
    let then_text = text.get(then_span.start..then_span.end)?;
    let else_text = text.get(else_span.start..else_span.end)?;

    let inverted_cond = invert_condition(cond_text);

    // Replace the entire if expression
    let new_text = format!("if {} then {} else {}", inverted_cond, else_text, then_text);
    Some(vec![IdeTextEdit { range: base_db::text_range(expr_span.start, expr_span.end), new_text }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            invert_if,
            "val f : int -> int\nfunction f(x) = $0if x > 0 then 1 else 2\n",
            expect![[r#"
            val f : int -> int
            function f(x) = if x <= 0 then 2 else 1
        "#]],
        );
    }
}
