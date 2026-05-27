//! `convert_comment_style` assist.
//!
//! Convert regular comment to doc comment and vice versa.
//! Before: `// This is a helper`
//! After:  `/** This is a helper */`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn convert_comment_style(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::convert_comment_style_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("convert_comment_style", AssistKind::RefactorRewrite),
        "Convert comment style",
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
        let labels = check_assist(convert_comment_style, "// This is a helper\n", 0);
        let _ = labels; // verify no panic
    }
}
