//! Postfix completion provider — `expr.if`, `expr.match`, etc.
//! B5-2: Type-filtered postfix — templates are filtered by receiver
//! type category when type information is available.

use super::Completions;
#[cfg(test)]
use crate::completions::dot::extract_receiver_expr;
use crate::context::CompletionContext;
use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;

/// Complete postfix templates after `.`, with type-aware filtering.
pub(crate) fn complete_postfix(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    if !ctx.config.enable_postfix_completions {
        return;
    }
    let Some((receiver_start, receiver_text)) = ctx
        .offset
        .checked_sub(ctx.prefix.len() + 1)
        .filter(|&dot| ctx.text.as_bytes().get(dot) == Some(&b'.'))
        .and_then(|dot| ctx.postfix_receiver(dot))
    else {
        return;
    };
    let mut items =
        postfix_for_receiver(ctx.text, ctx.offset, ctx.prefix, receiver_start, receiver_text);
    if ctx.config.enable_snippets {
        items.retain(|item| {
            !ctx.config
                .custom_templates
                .iter()
                .any(|template| template.postfix.as_deref() == item.filter_text.as_deref())
        });
    }

    // B5-2: Try to determine receiver type for filtering.
    // Extract receiver span and look up its type via FileDb.
    if !receiver_text.is_empty() {
        // Look up receiver type from binding type cache
        let receiver_span = parser::Span::new(receiver_start, receiver_start + receiver_text.len());
        let type_text = ctx
            .file
            .file_text()
            .and_then(|(db, ft)| hir::Semantics::new(db).binding_type_text(ft, receiver_span))
            .or_else(|| {
                ctx.file
                    .file_text()
                    .and_then(|(db, ft)| hir::Semantics::new(db).expr_type_text(ft, receiver_span))
            });

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

    acc.add_many(items);
}

/// Generate postfix completions (e.g. `expr.if` -> `if expr then { }`)
#[cfg(test)]
pub(crate) fn postfix_completions(text: &str, offset: usize, prefix: &str) -> Vec<CompletionItem> {
    // Find the dot before the prefix
    let prefix_start = offset - prefix.len();
    if prefix_start == 0 {
        return Vec::new();
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return Vec::new();
    }

    // Extract the receiver expression (text before the dot)
    let dot_pos = prefix_start - 1;
    let receiver = extract_receiver_expr(text, dot_pos);
    if receiver.is_empty() {
        return Vec::new();
    }

    postfix_for_receiver(
        text,
        offset,
        prefix,
        text[..dot_pos].trim_end().len() - receiver.len(),
        receiver,
    )
}

fn postfix_for_receiver(
    text: &str,
    offset: usize,
    prefix: &str,
    receiver_start: usize,
    receiver: &str,
) -> Vec<CompletionItem> {
    let prefix_lower = prefix.to_ascii_lowercase();

    let postfix_templates: &[(&str, &str, &str)] = &[
        ("if", "if {} then {{\n\t$0\n}}", "Wrap in if-then"),
        ("match", "match {} {{\n\t${{1:_}} => $0\n}}", "Wrap in match"),
        ("let", "let ${{1:x}} = {}", "Bind to let"),
        ("not", "~({})", "Negate expression"),
        ("return", "return {}", "Return expression"),
        ("dbg", "/* DBG */ {}", "Debug wrapper"),
        ("foreach", "foreach (${{1:i}} from ${{2:0}} to {}) {{\n\t$0\n}}", "Wrap in foreach"),
        ("assert", "assert({}, ${{1:\"assertion failed\"}})", "Wrap in assert"),
        ("while", "while {} do {{\n\t$0\n}}", "Wrap in while-do"),
        ("var", "var ${{1:x}} = {}", "Bind to var (mutable)"),
        ("throw", "throw {}", "Throw expression"),
        ("exit", "exit({})", "Exit with expression"),
        ("some", "Some({})", "Wrap in Some"),
        ("unsigned", "unsigned({})", "Convert to unsigned"),
        ("signed", "signed({})", "Convert to signed"),
    ];

    let mut items = Vec::new();
    for (trigger, template, detail) in postfix_templates {
        if !trigger.starts_with(&prefix_lower) && !prefix_lower.is_empty() {
            continue;
        }

        // Templates use doubled literal braces. Unescape before inserting the
        // receiver so braces belonging to the user's expression stay intact.
        let snippet = template
            .replace("{{", "{")
            .replace("}}", "}")
            .replace("{}", &super::custom::escape_literal(receiver));

        let mut item = CompletionItem::new(CompletionItemKind::Snippet, format!(".{trigger}"))
            .detail(*detail)
            .filter_text(*trigger)
            .insert_text(snippet)
            // A postfix template is asked for by name, so it sorts first.
            .sort_text(format!("0000_{trigger}"));
        let suffix_len = text[offset..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        item.source_range = Some(base_db::text_range(receiver_start, offset + suffix_len));
        items.push(item);
    }
    items
}
