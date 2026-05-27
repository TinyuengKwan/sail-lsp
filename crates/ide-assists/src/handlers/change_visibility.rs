//! `change_visibility` assist — toggle private modifier.
//!
//! Sail uses `private` keyword instead of `pub`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn change_visibility(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.file.text();
    let offset = ctx.offset();

    // Check if we're on or near a "private" keyword
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text
        [line_start..text[line_start..].find('\n').map(|i| line_start + i).unwrap_or(text.len())];
    let trimmed = line.trim_start();

    if trimmed.starts_with("private ") {
        // Remove "private "
        let priv_start = line_start + line.find("private ").unwrap();
        let range = base_db::text_range(priv_start, priv_start + 8); // "private "
        acc.add_with_edits(
            AssistId("change_visibility", AssistKind::RefactorRewrite),
            "Make public (remove `private`)",
            ctx.range,
            vec![TextEdit { range, new_text: String::new() }],
        );
        Some(())
    } else if trimmed.starts_with("function ")
        || trimmed.starts_with("val ")
        || trimmed.starts_with("type ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("union ")
    {
        // Add "private " before the keyword
        let kw_start = line_start + (line.len() - trimmed.len());
        let range = base_db::text_range(kw_start, kw_start);
        acc.add_with_edits(
            AssistId("change_visibility", AssistKind::RefactorRewrite),
            "Make private",
            ctx.range,
            vec![TextEdit { range, new_text: "private ".to_string() }],
        );
        Some(())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(change_visibility, "function foo() -> unit = ()\n", 0);
        let _ = labels;
    }
}
