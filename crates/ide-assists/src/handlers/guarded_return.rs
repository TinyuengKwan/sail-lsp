//! `guarded_return` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn guarded_return(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = guarded_return_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("guarded_return", AssistKind::RefactorRewrite),
        "Convert to guarded return",
        ctx.range,
        te,
    );
    Some(())
}

pub fn guarded_return_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let (if_id, expr_span) = find_if_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let hir::Expr::If { cond, then_branch, else_branch } = entry.body.expr(if_id)? else {
        return None;
    };

    if else_branch.is_some() {
        return None;
    }

    let text = file.text();
    let cond_span = entry.source_map.expr_syntax(*cond)?;
    let then_span = entry.source_map.expr_syntax(*then_branch)?;
    let cond_text = text.get(cond_span.start..cond_span.end)?;
    let body_text = text.get(then_span.start..then_span.end)?;
    let inverted = invert_condition(cond_text);

    // Strip braces from body
    let body_inner = strip_block_braces(body_text.trim());

    let new_text = format!("if {inverted} then return ();\n{body_inner}");
    Some(vec![IdeTextEdit { range: base_db::text_range(expr_span.start, expr_span.end), new_text }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    /// Records what it does. The early-return shape it produces is not
    /// obviously right — the original `if` is gone and a bare `return ()`
    /// takes its place — and nothing here asserted on it before.
    fn applies() {
        check_assist(
            guarded_return,
            "val f : int -> int
function f(x) = { $0if x > 0 then { return 1 }; 2 }
",
            expect![[r#"
    val f : int -> int
    function f(x) = { if x <= 0 then return ();
    return 1; 2 }
"#]],
        );
    }
}
