//! `inline_variable` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn inline_variable(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::inline_variable_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_variable", AssistKind::RefactorInline),
        "Inline variable",
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
        let labels = check_assist(inline_variable, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
