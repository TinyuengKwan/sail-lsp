//! `inline_const_as_literal` assist.
//!
//! Replace a constant reference with its literal value.
//! Given: `let SIZE = 32`
//! Before: `bits(SIZE)`
//! After:  `bits(32)`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn inline_const_as_literal(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::inline_const_as_literal_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_const_as_literal", AssistKind::RefactorInline),
        "Inline constant as literal",
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
        let labels = check_assist(inline_const_as_literal, "let SIZE = 32\nbits(SIZE)\n", 0);
        let _ = labels; // verify no panic
    }
}
