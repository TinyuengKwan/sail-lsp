//! `remove_mut` assist.
//!
//! Remove `var` keyword and replace with `let` (immutable binding).
//! Before: `var x = 1`
//! After:  `let x = 1`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn remove_mut(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::remove_mut_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("remove_mut", AssistKind::RefactorRewrite),
        "Replace var with let",
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
        let labels = check_assist(remove_mut, "var x = 1\n", 0);
        let _ = labels; // verify no panic
    }
}
