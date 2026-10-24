//! `evaluate_constant` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn evaluate_constant(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = evaluate_constant_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("evaluate_constant", AssistKind::RefactorRewrite),
        "Evaluate constant",
        ctx.range,
        te,
    );
    Some(())
}

/// Code action: evaluate a constant expression and replace with its value.
/// Replaces `2 + 3` with `5`, `0xFF` with `bits(8)`, etc.
pub fn evaluate_constant_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text().get(base_db::range_start(range)..base_db::range_end(range))?;
    let folded = try_fold_constant(text)?;
    // Don't offer if the result is the same as the input
    if folded.trim() == text.trim() {
        return None;
    }
    Some(vec![IdeTextEdit { range, new_text: folded }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(evaluate_constant, "let x : int = 2 + 3\n", 0);
        let _ = labels;
    }
}
