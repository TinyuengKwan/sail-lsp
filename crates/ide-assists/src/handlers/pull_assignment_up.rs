//! `pull_assignment_up` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn pull_assignment_up(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::pull_assignment_up_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("pull_assignment_up", AssistKind::RefactorRewrite),
        "Pull assignment up",
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
        let labels = check_assist(pull_assignment_up, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
