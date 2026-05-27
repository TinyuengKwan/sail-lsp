//! `unwrap_block` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn unwrap_block(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::unwrap_block_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("unwrap_block", AssistKind::RefactorRewrite),
        "Unwrap block",
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
        let labels = check_assist(unwrap_block, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
