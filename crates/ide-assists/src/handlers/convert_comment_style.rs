//! `convert_comment_style` assist.
//!
//! Convert regular comment to doc comment and vice versa.
//! Before: `// This is a helper`
//! After:  `/** This is a helper */`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn convert_comment_style(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = convert_comment_style_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("convert_comment_style", AssistKind::RefactorRewrite),
        "Convert comment style",
        ctx.range,
        te,
    );
    Some(())
}

/// Convert between regular comments (`//`) and block doc comments (`/** */`).
pub fn convert_comment_style_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let offset = base_db::range_start(range);

    // Find the line at the cursor.
    let line_start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |p| offset + p);
    let line = text.get(line_start..line_end)?;
    let trimmed = line.trim();

    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();

    if let Some(rest) = trimmed.strip_prefix("/** ") {
        // Block doc -> line comment
        let content = rest.strip_suffix(" */")?;
        let new_text = format!("{indent}// {content}");
        Some(vec![IdeTextEdit { range: base_db::text_range(line_start, line_end), new_text }])
    } else if trimmed.starts_with("//") && !trimmed.starts_with("///") {
        // Line comment -> block doc
        let content = trimmed.strip_prefix("//").unwrap_or(trimmed);
        let content = content.strip_prefix(' ').unwrap_or(content);
        let new_text = format!("{indent}/** {content} */");
        Some(vec![IdeTextEdit { range: base_db::text_range(line_start, line_end), new_text }])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(convert_comment_style, "// This is a helper\n", 0);
        let _ = labels; // verify no panic
    }
}
