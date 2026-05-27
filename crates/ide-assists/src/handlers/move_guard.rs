//! `move_guard` assist — move match guard to/from arm body.
//! Two assists:
//! 1. `move_guard_to_arm_body`: Convert `pat if guard => body` to
//!    `pat => if guard then body else ...`
//! 2. `move_arm_cond_to_match_guard`: Convert `pat => if cond then body`
//!    to `pat if cond => body`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;
use parser::{Span, Token};

/// Entry point: try both directions.
///
/// `move_arm_cond_to_match_guard` as separate handlers.
/// We combine them into one handler that checks both.
pub(crate) fn move_guard(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    move_guard_to_arm_body(acc, ctx).or_else(|| move_arm_cond_to_match_guard(acc, ctx))
}

/// Move a match arm guard into the arm body as an `if` expression.
/// ```sail
/// match x {
///   pat if guard => body,
/// }
/// ```
/// →
/// ```sail
/// match x {
///   pat => if guard then body else (),
/// }
/// ```
fn move_guard_to_arm_body(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let tokens = ctx.file.tokens()?;
    let offset = ctx.offset();

    // Find `if` keyword inside a match arm (after pattern, before `=>`)
    let (if_span, guard_text, fat_arrow_span, body_text) =
        find_guard_at_offset(tokens, text, offset)?;

    // Build the replacement: remove guard, wrap body in if
    let guard_start = if_span.start;
    let guard_end = fat_arrow_span.start; // Remove from `if` to `=>`

    // New body: `if guard then original_body else ()`
    let new_body = format!("if {guard_text} then {body_text} else ()");

    // Find the body range to replace
    let body_start = fat_arrow_span.end;
    let body_end = find_arm_body_end(tokens, text, fat_arrow_span.end)?;
    let body_range = base_db::text_range(body_start, body_end);

    let target = base_db::text_range(if_span.start, if_span.end);
    let edits = vec![
        // Remove the guard: `if guard ` between pattern and `=>`
        TextEdit { range: base_db::text_range(guard_start, guard_end), new_text: String::new() },
        // Replace the body with wrapped version
        TextEdit { range: body_range, new_text: format!(" {new_body}") },
    ];

    acc.add_with_edits(
        AssistId("move_guard_to_arm_body", AssistKind::RefactorRewrite),
        "Move guard to arm body",
        target,
        edits,
    );
    Some(())
}

/// Move an `if` condition from arm body to a match guard.
/// ```sail
/// match x {
///   pat => if cond then body else other,
/// }
/// ```
/// →
/// ```sail
/// match x {
///   pat if cond => body,
/// }
/// ```
fn move_arm_cond_to_match_guard(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let tokens = ctx.file.tokens()?;
    let offset = ctx.offset();

    // Find a match arm where the body starts with `if`
    let (fat_arrow_span, if_span, cond_text, then_body, _else_body) =
        find_if_body_at_offset(tokens, text, offset)?;

    // Build: insert guard before `=>`, replace body with then_body
    let target = base_db::text_range(if_span.start, if_span.end);

    // Insert `if cond ` before `=>`
    let guard_text = format!("if {cond_text} ");
    let body_start = fat_arrow_span.end;
    let body_end = find_arm_body_end(tokens, text, body_start)?;

    let edits = vec![
        // Insert guard before `=>`
        TextEdit {
            range: base_db::text_range(fat_arrow_span.start, fat_arrow_span.start),
            new_text: guard_text,
        },
        // Replace body (remove the if/then/else, keep just then_body)
        TextEdit {
            range: base_db::text_range(body_start, body_end),
            new_text: format!(" {then_body}"),
        },
    ];

    acc.add_with_edits(
        AssistId("move_arm_cond_to_match_guard", AssistKind::RefactorRewrite),
        "Move condition to match guard",
        target,
        edits,
    );
    Some(())
}

/// Find a guard `if cond` between a pattern and `=>` at the offset.
/// Returns (if_span, guard_text, fat_arrow_span, body_text).
fn find_guard_at_offset(
    tokens: &[(Token, Span)],
    text: &str,
    offset: usize,
) -> Option<(Span, String, Span, String)> {
    // Look for `if` keyword near offset that's followed by `=>`
    for (i, (tok, span)) in tokens.iter().enumerate() {
        if !matches!(tok, Token::KwIf) {
            continue;
        }
        if !(span.start <= offset + 20 && offset <= span.end + 100) {
            continue;
        }

        // Scan forward to find `=>`
        let mut _guard_end = span.end;
        let mut fat_arrow = None;
        for (tok2, span2) in &tokens[i + 1..] {
            if matches!(tok2, Token::FatRightArrow) {
                fat_arrow = Some(*span2);
                break;
            }
            _guard_end = span2.end;
        }
        let fat_arrow_span = fat_arrow?;

        // Verify this `if` is a guard (not a body `if`): check that
        // there's a pattern token before it (not `=>`)
        let mut is_guard = false;
        for (tok_before, _) in tokens[..i].iter().rev() {
            match tok_before {
                Token::FatRightArrow => break, // Body `if`, not guard
                Token::Id(_) | Token::Underscore | Token::KwTrue | Token::KwFalse => {
                    is_guard = true;
                    break;
                }
                _ => continue,
            }
        }
        if !is_guard {
            continue;
        }

        let guard_text = text[span.end..fat_arrow_span.start].trim().to_string();
        let body_start = fat_arrow_span.end;
        let body_end = find_arm_body_end(tokens, text, body_start)?;
        let body_text = text[body_start..body_end].trim().to_string();

        return Some((*span, guard_text, fat_arrow_span, body_text));
    }
    None
}

/// Find a match arm where the body starts with `if` at the offset.
/// Returns (fat_arrow_span, if_span, cond_text, then_body, else_body).
fn find_if_body_at_offset(
    tokens: &[(Token, Span)],
    text: &str,
    offset: usize,
) -> Option<(Span, Span, String, String, String)> {
    // Look for `=>` followed by `if` near offset
    for (i, (tok, span)) in tokens.iter().enumerate() {
        if !matches!(tok, Token::FatRightArrow) {
            continue;
        }
        if span.end + 100 < offset || span.start > offset + 100 {
            continue;
        }

        // Check if next non-whitespace token is `if`
        let mut if_span = None;
        for (tok2, span2) in &tokens[i + 1..] {
            if matches!(tok2, Token::KwIf) {
                if_span = Some(*span2);
                break;
            }
            // Allow only whitespace between `=>` and `if`
            if span2.start > span.end + 20 {
                break;
            }
        }
        let if_span = if_span?;

        // Find `then` keyword
        let mut then_span = None;
        for (tok2, span2) in tokens.iter() {
            if span2.start <= if_span.end {
                continue;
            }
            if matches!(tok2, Token::KwThen) {
                then_span = Some(*span2);
                break;
            }
        }
        let then_span = then_span?;

        let cond_text = text[if_span.end..then_span.start].trim().to_string();

        // Find `else` keyword and extract then_body
        let mut else_span = None;
        let mut depth = 0i32;
        for (tok2, span2) in tokens.iter() {
            if span2.start <= then_span.end {
                continue;
            }
            match tok2 {
                Token::KwIf => depth += 1,
                Token::KwElse if depth == 0 => {
                    else_span = Some(*span2);
                    break;
                }
                Token::KwElse => depth -= 1,
                _ => {}
            }
        }

        let (then_body, else_body) = if let Some(else_s) = else_span {
            let then_body = text[then_span.end..else_s.start].trim().to_string();
            let arm_end = find_arm_body_end(tokens, text, else_s.end)?;
            let else_body = text[else_s.end..arm_end].trim().to_string();
            (then_body, else_body)
        } else {
            let arm_end = find_arm_body_end(tokens, text, then_span.end)?;
            let then_body = text[then_span.end..arm_end].trim().to_string();
            (then_body, String::new())
        };

        return Some((*span, if_span, cond_text, then_body, else_body));
    }
    None
}

/// Find the end of a match arm body (up to `,` or `}` at depth 0).
fn find_arm_body_end(tokens: &[(Token, Span)], _text: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (tok, span) in tokens.iter() {
        if span.start < start {
            continue;
        }
        match tok {
            Token::LeftCurlyBracket | Token::LeftBracket => depth += 1,
            Token::RightCurlyBracket => {
                if depth == 0 {
                    return Some(span.start);
                }
                depth -= 1;
            }
            Token::RightBracket => depth -= 1,
            Token::Comma if depth == 0 => return Some(span.start),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(move_guard, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
