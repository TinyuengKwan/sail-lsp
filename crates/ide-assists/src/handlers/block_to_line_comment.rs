//! `block_to_line_comment` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn block_to_line_comment(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = block_to_line_comment_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("block_to_line_comment", AssistKind::RefactorRewrite),
        "Block to line comment",
        ctx.range,
        te,
    );
    Some(())
}

/// Convert block comments (/* */) to line comments (//)
pub fn block_to_line_comment_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let start_offset = base_db::range_start(range);
    let end_offset = base_db::range_end(range);

    let region = text.get(start_offset..end_offset)?.trim();
    if !region.starts_with("/*") || !region.ends_with("*/") {
        return None;
    }

    let inner = &region[2..region.len() - 2];
    let mut lines = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().strip_prefix("* ").or_else(|| line.trim().strip_prefix('*'));
        let content = trimmed.unwrap_or(line.trim());
        if content.is_empty() {
            continue;
        }
        lines.push(format!("// {content}"));
    }

    if lines.is_empty() {
        return None;
    }

    Some(vec![IdeTextEdit { range, new_text: lines.join("\n") }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(block_to_line_comment, "/* hello */\n", 0);
        let _ = labels;
    }
}
