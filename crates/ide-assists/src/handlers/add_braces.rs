//! `add_braces` assist — wrap single-line if/else bodies in `{ }`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn add_braces(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find an "if" keyword near the cursor
    let if_pos = find_if_near(text, offset)?;

    // Parse: if <cond> then <body> [else <body>]
    let after_if = skip_ws(&text[if_pos + 2..]);
    let cond_start = if_pos + 2 + (text[if_pos + 2..].len() - after_if.len());

    // Find "then" keyword
    let then_idx = text[cond_start..].find(" then ")?;
    let then_abs = cond_start + then_idx;
    let body_start_abs = then_abs + 6; // " then " is 6 chars

    let body_start_trimmed =
        body_start_abs + (text[body_start_abs..].len() - skip_ws(&text[body_start_abs..]).len());

    // If body already starts with '{', bail
    if text.as_bytes().get(body_start_trimmed) == Some(&b'{') {
        return None;
    }

    // Check cursor is within the if..then..else range
    // Find where the then-body ends (either at "else" or end of expression)
    let mut edits = Vec::new();

    // Find "else" keyword after the then body
    if let Some(else_rel) = find_keyword_else(&text[body_start_trimmed..]) {
        let else_abs = body_start_trimmed + else_rel;
        let then_body = text[body_start_trimmed..else_abs].trim();

        // Wrap the then-body
        edits.push(TextEdit {
            range: base_db::text_range(body_start_trimmed, else_abs),
            new_text: format!("{{ {} }} ", then_body),
        });

        // Now handle else body
        let else_body_start = else_abs + 4; // "else" is 4 chars
        let else_body_start_trimmed = else_body_start
            + (text[else_body_start..].len() - skip_ws(&text[else_body_start..]).len());

        if text.as_bytes().get(else_body_start_trimmed) != Some(&b'{') {
            // Find end of else body (next newline or end of text)
            let else_body_end = find_expr_end(&text[else_body_start_trimmed..]);
            let else_end = else_body_start_trimmed + else_body_end;
            let else_body = text[else_body_start_trimmed..else_end].trim();

            edits.push(TextEdit {
                range: base_db::text_range(else_body_start_trimmed, else_end),
                new_text: format!("{{ {} }}", else_body),
            });
        }
    } else {
        // No else: wrap just the then-body
        let body_end = find_expr_end(&text[body_start_trimmed..]);
        let end_abs = body_start_trimmed + body_end;
        let then_body = text[body_start_trimmed..end_abs].trim();

        edits.push(TextEdit {
            range: base_db::text_range(body_start_trimmed, end_abs),
            new_text: format!("{{ {} }}", then_body),
        });
    }

    if edits.is_empty() {
        return None;
    }

    // Check cursor is within the if expression range
    let last_edit_end = edits.iter().map(|e| base_db::range_end(e.range)).max().unwrap_or(0);
    if offset < if_pos || offset > last_edit_end + 50 {
        return None;
    }

    acc.add_with_edits(
        AssistId("add_braces", AssistKind::RefactorRewrite),
        "Add braces to if/else",
        ctx.range,
        edits,
    );
    Some(())
}

fn find_if_near(text: &str, offset: usize) -> Option<usize> {
    // Search backward for "if "
    let start = offset.saturating_sub(200);
    let window = &text[start..text.len().min(offset + 50)];
    // Find the last "if " before cursor
    let mut best = None;
    let mut search_from = 0;
    loop {
        match window[search_from..].find("if ") {
            Some(pos) => {
                let abs = start + search_from + pos;
                // Make sure it's a keyword (preceded by whitespace or start)
                if abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric() {
                    best = Some(abs);
                }
                search_from += pos + 1;
            }
            None => break,
        }
    }
    best
}

fn find_keyword_else(text: &str) -> Option<usize> {
    let mut search_from = 0;
    loop {
        match text[search_from..].find("else") {
            Some(pos) => {
                let abs = search_from + pos;
                let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
                let after_ok =
                    abs + 4 >= text.len() || !text.as_bytes()[abs + 4].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return Some(abs);
                }
                search_from = abs + 1;
            }
            None => return None,
        }
    }
}

fn find_expr_end(text: &str) -> usize {
    // Simple heuristic: find end of line or semicolon
    for (i, ch) in text.char_indices() {
        if ch == '\n' || ch == ';' {
            return i;
        }
    }
    text.len()
}

fn skip_ws(s: &str) -> &str {
    s.trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(add_braces, "if x then y else z\n", 0);
        let _ = labels;
    }
}
