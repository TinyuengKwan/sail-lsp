//! `generate_constant` assist.
//!
//! Extract a magic number into a named constant.
//! Before: `bits(32)`
//! After:  `let XLEN = 32; bits(XLEN)`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_constant(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::generate_constant_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("generate_constant", AssistKind::RefactorExtract),
        "Extract into constant",
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
        let labels = check_assist(generate_constant, "bits(32)\n", 0);
        let _ = labels; // verify no panic
    }
}
