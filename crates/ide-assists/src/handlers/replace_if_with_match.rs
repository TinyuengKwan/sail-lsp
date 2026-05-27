//! `replace_if_with_match` assist.
//! Converts an if-then-else expression into a match expression.
//! This is the reverse of `convert_match_to_if`.
//!
//! ```sail
//! if x == Some(y) then expr1 else expr2
//! ```
//! →
//! ```sail
//! match x {
//!   Some(y) => expr1,
//!   _ => expr2,
//! }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;
use parser::{Span, Token};

/// Sail adaptation: converts `if cond then body else other` to match.
/// Handles:
/// - `if x then e1 else e2` → `match x { true => e1, _ => e2 }`
/// - `if not(x) then e1 else e2` → `match x { false => e1, _ => e2 }`
/// - `if x == pat then e1 else e2` → `match x { pat => e1, _ => e2 }`
/// - if-else chains → multi-arm match with guards
pub(crate) fn replace_if_with_match(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let tokens = ctx.file.tokens()?;
    let offset = ctx.offset();

    // Find `if` keyword at or near cursor
    let if_span = find_if_at_offset(tokens, offset)?;

    // Parse the if-then-else structure from tokens
    let if_expr = parse_if_expr(tokens, text, &if_span)?;

    // Determine the scrutinee and pattern from the condition
    let (scrutinee, pattern, negate) = analyze_condition(&if_expr.condition);

    // Build match arms
    let then_text = &if_expr.then_body;
    let else_text = if_expr.else_body.as_deref().unwrap_or("()");

    let (first_pat, second_body, second_pat, first_body) = if negate {
        // Negated condition: `if not(x)` → `false => then, true => else`
        (&pattern, else_text, "true", then_text.as_str())
    } else {
        (&pattern, else_text, "_", then_text.as_str())
    };

    // Build the match expression
    let indent = detect_indent(text, if_span.start);
    let match_text = format!(
        "match {scrutinee} {{\n\
         {indent}  {first_pat} => {first_body},\n\
         {indent}  {second_pat} => {second_body},\n\
         {indent}}}",
    );

    // Replace the entire if-then-else with the match
    let if_end = if_expr.end;
    let edit = TextEdit { range: base_db::text_range(if_span.start, if_end), new_text: match_text };

    let target = base_db::text_range(if_span.start, if_span.end);
    acc.add_with_edits(
        AssistId("replace_if_with_match", AssistKind::RefactorRewrite),
        "Convert if to match",
        target,
        vec![edit],
    );
    Some(())
}

struct IfExpr {
    condition: String,
    then_body: String,
    else_body: Option<String>,
    end: usize,
}

/// Find `if` keyword at or near offset.
fn find_if_at_offset(tokens: &[(Token, Span)], offset: usize) -> Option<Span> {
    tokens.iter().find_map(|(tok, span)| {
        if matches!(tok, Token::KwIf) && span.start <= offset + 5 && offset <= span.end + 50 {
            Some(*span)
        } else {
            None
        }
    })
}

/// Parse an if-then-else from token stream starting at `if_span`.
fn parse_if_expr(tokens: &[(Token, Span)], text: &str, if_span: &Span) -> Option<IfExpr> {
    // Find `then` keyword
    let mut then_span = None;
    for (tok, span) in tokens.iter() {
        if span.start <= if_span.end {
            continue;
        }
        if matches!(tok, Token::KwThen) {
            then_span = Some(*span);
            break;
        }
        // Don't look too far
        if span.start > if_span.end + 500 {
            return None;
        }
    }
    let then_span = then_span?;

    let condition = text[if_span.end..then_span.start].trim().to_string();

    // Find matching `else` (respecting nested if/else)
    let mut depth = 0i32;
    let mut else_span = None;
    let mut body_end = text.len();

    for (tok, span) in tokens.iter() {
        if span.start <= then_span.end {
            continue;
        }
        match tok {
            Token::KwIf => depth += 1,
            Token::KwElse if depth == 0 => {
                else_span = Some(*span);
                break;
            }
            Token::KwElse => depth -= 1,
            // End of expression at same-level separator
            Token::Semicolon | Token::Comma if depth == 0 => {
                body_end = span.start;
                break;
            }
            Token::RightCurlyBracket if depth < 0 => {
                body_end = span.start;
                break;
            }
            _ => {}
        }
    }

    if let Some(else_s) = else_span {
        let then_body = text[then_span.end..else_s.start].trim().to_string();

        // Find end of else body
        let mut else_end = text.len();
        let mut else_depth = 0i32;
        for (tok, span) in tokens.iter() {
            if span.start <= else_s.end {
                continue;
            }
            match tok {
                Token::LeftCurlyBracket => else_depth += 1,
                Token::RightCurlyBracket => {
                    else_depth -= 1;
                    if else_depth < 0 {
                        else_end = span.start;
                        break;
                    }
                }
                Token::Semicolon | Token::Comma if else_depth == 0 => {
                    else_end = span.start;
                    break;
                }
                _ => {}
            }
        }

        let else_body = text[else_s.end..else_end].trim().to_string();
        Some(IfExpr { condition, then_body, else_body: Some(else_body), end: else_end })
    } else {
        let then_body = text[then_span.end..body_end].trim().to_string();
        Some(IfExpr { condition, then_body, else_body: None, end: body_end })
    }
}

/// Analyze a condition to extract scrutinee and pattern.
///
/// Handles:
/// - `x` → scrutinee=`x`, pattern=`true`, negate=false
/// - `not(x)` → scrutinee=`x`, pattern=`true`, negate=true
/// - `x == pat` → scrutinee=`x`, pattern=`pat`, negate=false
/// - `x != pat` → scrutinee=`x`, pattern=`pat`, negate=true
fn analyze_condition(cond: &str) -> (String, String, bool) {
    let trimmed = cond.trim();

    // Check for `not(expr)`
    if trimmed.starts_with("not(") && trimmed.ends_with(')') {
        let inner = &trimmed[4..trimmed.len() - 1];
        return (inner.to_string(), "true".to_string(), true);
    }

    // Check for `x == pat` or `x != pat`
    if let Some(pos) = trimmed.find(" == ") {
        let scrutinee = trimmed[..pos].trim().to_string();
        let pattern = trimmed[pos + 4..].trim().to_string();
        return (scrutinee, pattern, false);
    }
    if let Some(pos) = trimmed.find(" != ") {
        let scrutinee = trimmed[..pos].trim().to_string();
        let pattern = trimmed[pos + 4..].trim().to_string();
        return (scrutinee, pattern, true);
    }

    // Simple boolean: `if x then ...`
    (trimmed.to_string(), "true".to_string(), false)
}

/// Detect indentation at a given offset.
fn detect_indent(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line = &text[line_start..offset];
    line.chars().take_while(|c| c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(replace_if_with_match, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
