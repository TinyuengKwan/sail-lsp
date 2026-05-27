//! `block_to_line_comment` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn block_to_line_comment(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::block_to_line_comment_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("block_to_line_comment", AssistKind::RefactorRewrite),
        "Block to line comment",
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
        let labels = check_assist(block_to_line_comment, "/* hello */\n", 0);
        let _ = labels;
    }
}
