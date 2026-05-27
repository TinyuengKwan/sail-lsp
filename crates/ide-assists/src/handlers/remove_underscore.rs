//! `remove_underscore` assist — remove leading underscore from a variable name.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn remove_underscore(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find the identifier at the cursor
    let (word_start, word_end) = find_ident_at(text, offset)?;
    let ident = &text[word_start..word_end];

    // Must start with '_' and have at least one more character
    if !ident.starts_with('_') || ident.len() < 2 {
        return None;
    }

    // The rest after underscore must be a valid identifier (not just more underscores becoming empty)
    let without_underscore = &ident[1..];
    if without_underscore.is_empty() {
        return None;
    }

    // Check it's in a binding context (preceded by "let" or similar)
    // Simple heuristic: look back for "let" keyword
    let before = text[..word_start].trim_end();
    let is_binding = before.ends_with("let")
        || before.ends_with(':')
        || before.ends_with('(')
        || before.ends_with(',');

    if !is_binding {
        return None;
    }

    acc.add_with_edits(
        AssistId("remove_underscore", AssistKind::RefactorRewrite),
        "Remove leading underscore",
        ctx.range,
        vec![TextEdit {
            range: base_db::text_range(word_start, word_start + 1),
            new_text: String::new(),
        }],
    );
    Some(())
}

fn find_ident_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset >= text.len() {
        return None;
    }
    let bytes = text.as_bytes();

    // Check cursor is on an identifier character
    if !is_ident_char(bytes[offset]) {
        return None;
    }

    // Walk backward to start of identifier
    let mut start = offset;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }

    // Walk forward to end of identifier
    let mut end = offset;
    while end < text.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    Some((start, end))
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(remove_underscore, "let _x = 1\n", 4);
        let _ = labels;
    }
}
