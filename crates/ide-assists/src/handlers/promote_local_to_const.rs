//! `promote_local_to_const` assist.
//!
//! Promote a local let binding to a top-level definition.
//! Before (inside function): `let width = 32`
//! After (top-level): `let width : int = 32`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn promote_local_to_const(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::promote_local_to_const_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("promote_local_to_const", AssistKind::RefactorExtract),
        "Promote to top-level constant",
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
        let labels = check_assist(promote_local_to_const, "function f() = { let width = 32 }\n", 0);
        let _ = labels; // verify no panic
    }
}
