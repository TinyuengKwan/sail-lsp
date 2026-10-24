//! `apply_demorgan` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn apply_demorgan(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = apply_demorgan_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("apply_demorgan", AssistKind::RefactorRewrite),
        "Apply De Morgan's law",
        ctx.range,
        te,
    );
    Some(())
}

pub fn apply_demorgan_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let (infix_id, expr_span) = find_infix_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let hir::Expr::BinaryOp { lhs, op, rhs } = entry.body.expr(infix_id)? else {
        return None;
    };

    let new_op = match op.as_str() {
        "&" => "|",
        "|" => "&",
        _ => return None,
    };

    let text = file.text();
    let lhs_span = entry.source_map.expr_syntax(*lhs)?;
    let rhs_span = entry.source_map.expr_syntax(*rhs)?;
    let lhs_text = text.get(lhs_span.start..lhs_span.end)?;
    let rhs_text = text.get(rhs_span.start..rhs_span.end)?;

    let inv_lhs = invert_condition(lhs_text);
    let inv_rhs = invert_condition(rhs_text);

    let new_text = format!("~({inv_lhs} {new_op} {inv_rhs})");
    Some(vec![IdeTextEdit { range: base_db::text_range(expr_span.start, expr_span.end), new_text }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(apply_demorgan, "val x : bool\n", 0);
        let _ = labels;
    }
}
