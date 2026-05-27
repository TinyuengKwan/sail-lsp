//! `add_explicit_type` assist.
//! Adds an explicit type annotation to a `let` or `var` binding
//! when the type can be inferred.
//!
//! ```sail
//! let x = 42
//! ```
//! →
//! ```sail
//! let x : int = 42
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::{Span, Token};

/// Triggers when cursor is on a `let`/`var` binding name that has no
/// type annotation. Uses `FileDb::binding_type_text()` to get the
/// inferred type from the salsa query.
pub(crate) fn add_explicit_type(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let tokens = ctx.file.tokens()?;
    let offset = ctx.offset();

    // Find the let/var keyword before cursor
    let (_let_kind, let_span) = find_let_var_at_offset(tokens, offset)?;

    // Find the binding name (identifier after let/var)
    let (_name_text, name_span) = find_name_after_let(tokens, text, &let_span)?;

    // Check cursor is on or near the binding name
    if offset > name_span.end + 1 {
        return None;
    }

    // Check if there's already a type annotation (`:` between name and `=`)
    if has_type_annotation(tokens, &name_span) {
        return None; // Already has explicit type
    }

    // Get inferred type from salsa query
    let ty_text = ctx.file.binding_type_text(name_span)?;
    if ty_text == "unknown" || ty_text == "error" || ty_text == "unit" {
        return None;
    }

    // Insert `: type` after the binding name
    let insert_offset = name_span.end;
    let insert_text = format!(": {ty_text}");

    let target = base_db::text_range(name_span.start, name_span.end);
    acc.add(
        AssistId("add_explicit_type", AssistKind::RefactorRewrite),
        format!("Insert explicit type `{ty_text}`"),
        target,
        |builder| {
            builder.insert(insert_offset, insert_text.clone());
        },
    )
}

/// Find a `let` or `var` keyword at or before the given offset.
fn find_let_var_at_offset(tokens: &[(Token, Span)], offset: usize) -> Option<(&Token, Span)> {
    // Scan backward from offset to find the nearest let/var
    for (tok, span) in tokens.iter().rev() {
        if span.start > offset {
            continue;
        }
        if span.end + 200 < offset {
            break; // Too far back
        }
        match tok {
            Token::KwLet | Token::KwVar => return Some((tok, *span)),
            _ => {}
        }
    }
    None
}

/// Find the identifier token after a let/var keyword.
fn find_name_after_let<'a>(
    tokens: &[(Token, Span)],
    text: &'a str,
    let_span: &Span,
) -> Option<(&'a str, Span)> {
    for (tok, span) in tokens.iter() {
        if span.start <= let_span.end {
            continue;
        }
        if let Token::Id(_) = tok {
            let name = &text[span.start..span.end];
            return Some((name, *span));
        }
        // Skip tokens between let and name (whitespace is not emitted as a token;
        // the lexer skips it). If we hit a non-Id token, stop.
        // Allow a small gap for whitespace between let and the name.
        if span.start > let_span.end + 50 {
            return None;
        }
    }
    None
}

/// Check if there's a `:` between the name and the next `=`.
fn has_type_annotation(tokens: &[(Token, Span)], name_span: &Span) -> bool {
    for (tok, span) in tokens.iter() {
        if span.start <= name_span.end {
            continue;
        }
        match tok {
            Token::Colon => return true,
            Token::Equal => return false,
            _ => continue,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    // Tests will be added when the full test infrastructure is wired.
}
