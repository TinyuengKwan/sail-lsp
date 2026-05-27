//! `evaluate_constant` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn evaluate_constant(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::evaluate_constant_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("evaluate_constant", AssistKind::RefactorRewrite),
        "Evaluate constant",
        ctx.range,
        te,
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(evaluate_constant, "let x : int = 2 + 3\n", 0);
        let _ = labels;
    }
}
