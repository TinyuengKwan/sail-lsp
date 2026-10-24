//! `pull_assignment_up` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn pull_assignment_up(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = pull_assignment_up_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("pull_assignment_up", AssistKind::RefactorRewrite),
        "Pull assignment up",
        ctx.range,
        te,
    );
    Some(())
}

pub fn pull_assignment_up_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    use hir::Expr;
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let text = file.text();

    // Find if-else at cursor where both branches are assignments to same target
    let (if_id, expr_span) = find_if_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let Expr::If { cond, then_branch, else_branch: Some(else_branch) } = entry.body.expr(if_id)?
    else {
        return None;
    };

    let then_span = entry.source_map.expr_syntax(*then_branch)?;
    let else_span = entry.source_map.expr_syntax(*else_branch)?;
    let cond_span = entry.source_map.expr_syntax(*cond)?;

    let then_text = text.get(then_span.start..then_span.end)?.trim();
    let else_text = text.get(else_span.start..else_span.end)?.trim();
    let then_inner = strip_block_braces(then_text);
    let else_inner = strip_block_braces(else_text);

    let (then_target, then_val) = split_assignment(then_inner)?;
    let (else_target, else_val) = split_assignment(else_inner)?;
    if then_target != else_target {
        return None;
    }

    let cond_text = text.get(cond_span.start..cond_span.end)?;
    let new_text = format!(
        "{} = if {} then {} else {}",
        then_target,
        cond_text,
        then_val.trim_end_matches(';').trim(),
        else_val.trim_end_matches(';').trim()
    );
    Some(vec![IdeTextEdit { range: base_db::text_range(expr_span.start, expr_span.end), new_text }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(pull_assignment_up, "val f : int -> unit\nfunction f(x) = { var y = 0; $0if x > 0 then { y = 1 } else { y = 2 } }\n", expect![[r#"
            val f : int -> unit
            function f(x) = { var y = 0; y = if x > 0 then 1 else 2 }
        "#]]);
    }
}
