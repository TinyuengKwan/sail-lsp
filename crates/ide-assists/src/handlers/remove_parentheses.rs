//! `remove_parentheses` assist — remove redundant parentheses.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn remove_parentheses(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.file.text();
    let offset = ctx.offset();
    let bytes = text.as_bytes();

    // Check if cursor is on a '(' or ')'
    if offset >= text.len() {
        return None;
    }
    let ch = bytes[offset];
    if ch != b'(' && ch != b')' {
        return None;
    }

    // Find matching paren
    let (open, close) = if ch == b'(' {
        let mut depth = 1i32;
        let mut i = offset + 1;
        while i < text.len() && depth > 0 {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        if depth != 0 {
            return None;
        }
        (offset, i - 1)
    } else {
        let mut depth = 1i32;
        let mut i = offset;
        while i > 0 && depth > 0 {
            i -= 1;
            match bytes[i] {
                b')' => depth += 1,
                b'(' => depth -= 1,
                _ => {}
            }
        }
        if depth != 0 {
            return None;
        }
        (i, offset)
    };

    let inner = &text[open + 1..close];
    let range = base_db::text_range(open, close + 1);

    acc.add_with_edits(
        AssistId("remove_parentheses", AssistKind::RefactorRewrite),
        "Remove parentheses",
        ctx.range,
        vec![TextEdit { range, new_text: inner.to_string() }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(remove_parentheses, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
