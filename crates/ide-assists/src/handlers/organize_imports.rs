//! `organize_imports` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn organize_imports(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::organize_imports_edits(ctx.file)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("organize_imports", AssistKind::Source),
        "Organize imports",
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
        let labels = check_assist(organize_imports, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
