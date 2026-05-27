//! `remove_unused_param` assist.
//! Removes a function parameter that is never used in the body.
//! Also updates all call sites to remove the corresponding argument.
//!
//! ```sail
//! function foo(x : int, y : int) -> int = x
//! ```
//! → (cursor on `y`)
//! ```sail
//! function foo(x : int) -> int = x
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use parser::{Span, Token};

/// Detects when cursor is on a function parameter that is never referenced
/// in the function body. Offers to remove the parameter from the signature.
///
/// Note: call site update requires workspace-wide search (FindUsages).
/// This implementation handles the signature edit only. Call site updates
/// will be added when `Definition::usages()` is fully wired.
pub(crate) fn remove_unused_param(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let tokens = ctx.file.tokens()?;
    let offset = ctx.offset();

    // Find the parameter name at cursor — must be inside a function signature
    let (param_name, param_span) = find_param_at_offset(tokens, text, offset)?;

    // Find the enclosing function
    let (_fn_name, fn_body_start, fn_body_end) = find_enclosing_function(tokens, text, offset)?;

    // Check if the parameter name is used in the function body
    let body_text = &text[fn_body_start..fn_body_end.min(text.len())];
    if is_name_used_in_body(body_text, &param_name) {
        return None; // Parameter is used
    }

    // Find the parameter range including surrounding comma/whitespace
    let param_range = find_param_range_with_separator(text, tokens, &param_span)?;

    let target = base_db::text_range(param_span.start, param_span.end);
    acc.add_with_edits(
        AssistId("remove_unused_param", AssistKind::RefactorRewrite),
        format!("Remove unused parameter `{param_name}`"),
        target,
        vec![TextEdit { range: param_range, new_text: String::new() }],
    );
    Some(())
}

/// Find a parameter name (identifier) at the given offset that's
/// inside a function parameter list (between `(` and `)`).
fn find_param_at_offset(
    tokens: &[(Token, Span)],
    text: &str,
    offset: usize,
) -> Option<(String, Span)> {
    // Find the Id token at offset
    let (_, id_span) = tokens.iter().find(|(tok, span)| {
        matches!(tok, Token::Id(_)) && span.start <= offset && offset <= span.end
    })?;

    // Verify it's inside a parameter list: scan backward for `(`
    let mut paren_depth = 0i32;
    let mut in_params = false;
    for (tok, span) in tokens.iter().rev() {
        if span.start > id_span.start {
            continue;
        }
        match tok {
            Token::RightBracket => paren_depth += 1,
            Token::LeftBracket => {
                if paren_depth == 0 {
                    in_params = true;
                    break;
                }
                paren_depth -= 1;
            }
            Token::KwFunction => break, // Went past function keyword
            _ => {}
        }
    }

    if !in_params {
        return None;
    }

    let name = text[id_span.start..id_span.end].to_string();
    Some((name, *id_span))
}

/// Find the enclosing function's name, body start, and body end.
fn find_enclosing_function(
    tokens: &[(Token, Span)],
    text: &str,
    offset: usize,
) -> Option<(String, usize, usize)> {
    // Scan backward for `function` keyword
    let mut fn_span = None;
    for (tok, span) in tokens.iter() {
        if span.start > offset {
            break;
        }
        if matches!(tok, Token::KwFunction) {
            fn_span = Some(*span);
        }
    }
    let fn_span = fn_span?;

    // Get function name (next Id after `function`)
    let fn_name = tokens
        .iter()
        .find(|(tok, span)| matches!(tok, Token::Id(_)) && span.start > fn_span.end)
        .map(|(_, span)| text[span.start..span.end].to_string())?;

    // Find function body: between `=` and end of function
    let mut body_start = None;
    let mut brace_depth = 0i32;
    let mut body_end = text.len();

    for (tok, span) in tokens.iter() {
        if span.start < fn_span.start {
            continue;
        }
        if body_start.is_none() {
            if matches!(tok, Token::Equal) && span.start > fn_span.end {
                body_start = Some(span.end);
            }
            continue;
        }
        match tok {
            Token::LeftCurlyBracket => brace_depth += 1,
            Token::RightCurlyBracket => {
                brace_depth -= 1;
                if brace_depth < 0 {
                    body_end = span.end;
                    break;
                }
            }
            Token::KwFunction if brace_depth <= 0 => {
                body_end = span.start;
                break;
            }
            _ => {}
        }
    }

    Some((fn_name, body_start?, body_end))
}

/// Check if a name appears as an identifier in body text.
fn is_name_used_in_body(body_text: &str, name: &str) -> bool {
    // Simple word-boundary check
    let bytes = body_text.as_bytes();
    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len();

    for i in 0..body_text.len().saturating_sub(name_len - 1) {
        if &bytes[i..i + name_len] == name_bytes {
            // Check word boundaries
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_ok = i + name_len >= bytes.len() || !is_ident_char(bytes[i + name_len]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\''
}

/// Find the full range of a parameter including its type annotation
/// and surrounding separator (comma or leading/trailing whitespace).
fn find_param_range_with_separator(
    _text: &str,
    tokens: &[(Token, Span)],
    param_span: &Span,
) -> Option<TextRange> {
    // Find the previous comma or `(` and next comma or `)`
    let mut prev_sep = None;
    let mut next_sep = None;

    for (tok, span) in tokens.iter() {
        match tok {
            Token::Comma | Token::LeftBracket if span.end <= param_span.start => {
                prev_sep = Some(*span);
            }
            Token::Comma if span.start >= param_span.end && next_sep.is_none() => {
                next_sep = Some(*span);
                break;
            }
            Token::RightBracket if span.start >= param_span.end && next_sep.is_none() => {
                next_sep = Some(*span);
                break;
            }
            _ => {}
        }
    }

    // Also need to include the type annotation after the param name (`: type`)
    let mut type_end = param_span.end;
    let mut seen_colon = false;
    let mut paren_depth = 0i32;
    for (tok, span) in tokens.iter() {
        if span.start < param_span.end {
            continue;
        }
        match tok {
            Token::Colon if !seen_colon => {
                seen_colon = true;
            }
            Token::Comma | Token::RightBracket if paren_depth == 0 => {
                type_end = span.start;
                break;
            }
            Token::LeftBracket => paren_depth += 1,
            Token::RightBracket => paren_depth -= 1,
            _ => {}
        }
    }

    let prev = prev_sep?;
    // If previous separator is `(`, delete from param start to next comma
    if matches!(
        tokens.iter().find(|(_, s)| s.start == prev.start).map(|(t, _)| t),
        Some(Token::LeftBracket)
    ) {
        // First parameter: delete up to next comma (or just the param if only one)
        if let Some(next) = next_sep {
            if matches!(
                tokens.iter().find(|(_, s)| s.start == next.start).map(|(t, _)| t),
                Some(Token::Comma)
            ) {
                return Some(base_db::text_range(param_span.start, next.end));
            }
        }
        return Some(base_db::text_range(param_span.start, type_end));
    }

    // Not first parameter: delete from previous comma to type end
    Some(base_db::text_range(prev.start, type_end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(remove_unused_param, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
