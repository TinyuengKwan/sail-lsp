//! `toggle_doc_comment` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn toggle_doc_comment(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::toggle_doc_comment_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("toggle_doc_comment", AssistKind::RefactorRewrite),
        "Toggle doc comment",
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
        let labels = check_assist(toggle_doc_comment, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
