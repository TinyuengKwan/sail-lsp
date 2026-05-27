//! `generate_doc_template` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_doc_template(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::generate_doc_template_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("generate_doc_template", AssistKind::RefactorRewrite),
        "Generate doc template",
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
        let labels = check_assist(generate_doc_template, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
