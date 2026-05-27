//! `bitfield_accessors` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn bitfield_accessors(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::bitfield_accessor_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("bitfield_accessors", AssistKind::RefactorRewrite),
        "Generate bitfield accessors",
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
        let labels =
            check_assist(bitfield_accessors, "bitfield Foo : bits(8) = { x : 0 .. 7 }\n", 0);
        let _ = labels;
    }
}
