//! `remove_mut` assist.
//!
//! Remove `var` keyword and replace with `let` (immutable binding).
//! Before: `var x = 1`
//! After:  `let x = 1`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn remove_mut(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = remove_mut_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("remove_mut", AssistKind::RefactorRewrite),
        "Replace var with let",
        ctx.range,
        te,
    );
    Some(())
}

/// Replace `var` keyword with `let` for immutable binding.
pub fn remove_mut_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let offset = base_db::range_start(range);

    // Find the line at the cursor.
    let line_start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |p| offset + p);
    let line = text.get(line_start..line_end)?;
    let trimmed = line.trim();

    if !trimmed.starts_with("var ") {
        return None;
    }

    // Find the position of `var` on this line.
    let var_offset_in_line = line.find("var ")?;
    let abs_start = line_start + var_offset_in_line;
    let abs_end = abs_start + 3; // length of "var"

    Some(vec![IdeTextEdit {
        range: base_db::text_range(abs_start, abs_end),
        new_text: "let".to_string(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(remove_mut, "var x = 1\n", 0);
        let _ = labels; // verify no panic
    }
}
