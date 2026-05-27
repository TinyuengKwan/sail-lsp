//! `extract_function` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn extract_function(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::extract_function_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("extract_function", AssistKind::RefactorExtract),
        "Extract function",
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
        let labels = check_assist(extract_function, "function foo() -> unit = ()\n", 0);
        let _ = labels;
    }
}
