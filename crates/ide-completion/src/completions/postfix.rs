//! Postfix completion provider — `expr.if`, `expr.match`, etc.
//! B5-2: Type-filtered postfix — templates are filtered by receiver
//! type category when type information is available.

use super::Completions;
use crate::context::CompletionContext;

/// Complete postfix templates after `.`, with type-aware filtering.
pub(crate) fn complete_postfix(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let mut items = crate::postfix_completions(ctx.text, ctx.offset, ctx.prefix);

    // B5-2: Try to determine receiver type for filtering.
    // Extract receiver span and look up its type via FileDb.
    let prefix_start = ctx.offset.saturating_sub(ctx.prefix.len());
    if prefix_start > 0 && ctx.text.as_bytes().get(prefix_start - 1) == Some(&b'.') {
        let dot_pos = prefix_start - 1;
        let receiver_text = crate::extract_receiver_expr(ctx.text, dot_pos);
        if !receiver_text.is_empty() {
            // Look up receiver type from binding type cache
            let receiver_span =
                parser::Span::new(dot_pos.saturating_sub(receiver_text.len()), dot_pos);
            let type_text = ctx
                .file
                .binding_type_text(receiver_span)
                .or_else(|| ctx.file.cached_expr_type_text(receiver_span));

            if let Some(ty) = type_text {
                let ty_lower = ty.to_ascii_lowercase();
                // Filter templates by type category
                items.retain(|item| {
                    let trigger = item.filter_text.as_deref().unwrap_or("");
                    match trigger {
                        // Numeric/bitvector only
                        "unsigned" | "signed" => {
                            ty_lower.contains("bits")
                                || ty_lower.contains("int")
                                || ty_lower.contains("nat")
                                || ty_lower.contains("range")
                        }
                        // Boolean only
                        "if" | "not" => {
                            ty_lower == "bool" || ty_lower.contains("bool")
                                || ty_lower.contains("int") || ty_lower.contains("bits")
                                // Allow for any type (if is common pattern)
                                || trigger == "if"
                        }
                        // Match: useful for enum/union types
                        "match" => true, // always available
                        // Always available
                        _ => true,
                    }
                });
            }
        }
    }

    acc.add_many(items);
}
