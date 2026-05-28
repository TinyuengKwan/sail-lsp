//! `merge_nested_if` assist — merge nested `if` into a single `if` with `&` condition.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn merge_nested_if(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find outer "if" near cursor
    let outer_if = find_keyword_at(text, offset, "if")?;

    // Parse: if <cond1> then { if <cond2> then <body> }
    let after_outer_if = outer_if + 2;
    let rest = &text[after_outer_if..];

    // Find "then" for outer if
    let then_rel = find_word(rest, "then")?;
    let outer_cond = rest[..then_rel].trim();
    let after_then = after_outer_if + then_rel + 4; // "then" is 4 chars

    // Skip whitespace and expect '{'
    let body_start =
        after_then + (text[after_then..].len() - text[after_then..].trim_start().len());
    if text.as_bytes().get(body_start)? != &b'{' {
        return None;
    }

    // Find matching '}'
    let close_brace = find_matching_brace(text, body_start)?;

    // Inner content between braces
    let inner = text[body_start + 1..close_brace].trim();

    // Inner must start with "if"
    if !inner.starts_with("if ") {
        return None;
    }

    // Parse inner if
    let inner_rest = &inner[3..]; // skip "if "
    let inner_then = find_word(inner_rest, "then")?;
    let inner_cond = inner_rest[..inner_then].trim();
    let inner_body = inner_rest[inner_then + 4..].trim();

    // Build merged expression
    let merged_cond = format!("({} & {})", outer_cond, inner_cond);
    let merged = format!("if {} then {}", merged_cond, inner_body);

    // The full range to replace: from outer_if to close_brace + 1
    let end = close_brace + 1;

    acc.add_with_edits(
        AssistId("merge_nested_if", AssistKind::RefactorRewrite),
        "Merge nested if",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(outer_if, end), new_text: merged }],
    );
    Some(())
}

fn find_keyword_at(text: &str, offset: usize, kw: &str) -> Option<usize> {
    let start = offset.saturating_sub(100);
    let end = text.len().min(offset + 50);
    let window = &text[start..end];
    let mut search_from = 0;
    let mut best = None;
    let kw_len = kw.len();
    while let Some(pos) = window[search_from..].find(kw) {
        let abs = start + search_from + pos;
        let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
        let after_ok =
            abs + kw_len >= text.len() || !text.as_bytes()[abs + kw_len].is_ascii_alphanumeric();
        if before_ok && after_ok {
            best = Some(abs);
        }
        search_from += pos + 1;
    }
    best
}

fn find_word(text: &str, word: &str) -> Option<usize> {
    let mut search_from = 0;
    loop {
        match text[search_from..].find(word) {
            Some(pos) => {
                let abs = search_from + pos;
                let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
                let after_ok = abs + word.len() >= text.len()
                    || !text.as_bytes()[abs + word.len()].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return Some(abs);
                }
                search_from = abs + 1;
            }
            None => return None,
        }
    }
}

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut i = open + 1;
    let bytes = text.as_bytes();
    while i < text.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(merge_nested_if, "if x then { if y then z }\n", 0);
        let _ = labels;
    }
}
