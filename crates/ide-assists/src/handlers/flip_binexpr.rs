//! `flip_binexpr` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn flip_binexpr(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = flip_binexpr_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("flip_binexpr", AssistKind::RefactorRewrite),
        "Flip binary expression",
        ctx.range,
        te,
    );
    Some(())
}

pub fn flip_binexpr_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let (infix_id, expr_span) = find_infix_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let hir::Expr::BinaryOp { lhs, op, rhs } = entry.body.expr(infix_id)? else {
        return None;
    };

    let text = file.text();
    let lhs_span = entry.source_map.expr_syntax(*lhs)?;
    let rhs_span = entry.source_map.expr_syntax(*rhs)?;
    let lhs_text = text.get(lhs_span.start..lhs_span.end)?;
    let rhs_text = text.get(rhs_span.start..rhs_span.end)?;
    let op_text = op.as_str();

    let flipped_op = flip_comparison_op(op_text);

    let new_text = format!("{} {} {}", rhs_text, flipped_op, lhs_text);
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
            flip_binexpr,
            "val f : int -> bool\nfunction f(x) = x $0< 3\n",
            expect![[r#"
            val f : int -> bool
            function f(x) = 3 > x
        "#]],
        );
    }
}
