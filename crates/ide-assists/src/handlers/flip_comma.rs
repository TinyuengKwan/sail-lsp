//! `flip_comma` assist — swap items around a comma.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn flip_comma(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.file.text();
    let offset = ctx.offset();
    let bytes = text.as_bytes();

    // Find a comma at or near the cursor
    if offset >= text.len() || bytes[offset] != b',' {
        return None;
    }

    // Find the item before the comma (skip whitespace backwards)
    let mut before_end = offset;
    while before_end > 0 && bytes[before_end - 1] == b' ' {
        before_end -= 1;
    }
    let mut before_start = before_end;
    while before_start > 0
        && bytes[before_start - 1] != b','
        && bytes[before_start - 1] != b'('
        && bytes[before_start - 1] != b'{'
        && bytes[before_start - 1] != b'['
    {
        before_start -= 1;
    }
    while before_start < before_end && bytes[before_start] == b' ' {
        before_start += 1;
    }

    // Find the item after the comma (skip whitespace forwards)
    let mut after_start = offset + 1;
    while after_start < text.len() && bytes[after_start] == b' ' {
        after_start += 1;
    }
    let mut after_end = after_start;
    while after_end < text.len()
        && bytes[after_end] != b','
        && bytes[after_end] != b')'
        && bytes[after_end] != b'}'
        && bytes[after_end] != b']'
    {
        after_end += 1;
    }
    while after_end > after_start && bytes[after_end - 1] == b' ' {
        after_end -= 1;
    }

    if before_start >= before_end || after_start >= after_end {
        return None;
    }

    let before_text = &text[before_start..before_end];
    let after_text = &text[after_start..after_end];

    let range = base_db::text_range(before_start, after_end);
    let separator = &text[before_end..after_start]; // ", " or ","
    let new_text = format!("{after_text}{separator}{before_text}");

    acc.add_with_edits(
        AssistId("flip_comma", AssistKind::RefactorRewrite),
        "Flip comma",
        ctx.range,
        vec![TextEdit { range, new_text }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(flip_comma, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
