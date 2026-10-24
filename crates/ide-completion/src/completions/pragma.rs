//! Pragma completion provider — `@name` and `$name` directives.
//!
//! Completes known pragma names.

use super::Completions;
use crate::context::CompletionContext;
use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;

/// Complete pragma names after `@` or `$`.
pub(crate) fn complete_pragma(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let items = pragma_completions(ctx.text, ctx.offset);
    acc.add_many(items);
}

/// Pragma name completion: triggered when cursor is right after `@` or `$`.
pub(crate) fn pragma_completions(text: &str, offset: usize) -> Vec<CompletionItem> {
    if offset == 0 {
        return Vec::new();
    }
    let bytes = text.as_bytes();

    // Look back to find @ or $ that starts a pragma
    let mut start = offset;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start == 0 {
        return Vec::new();
    }

    let trigger = bytes[start - 1];
    if trigger != b'@' && trigger != b'$' {
        return Vec::new();
    }

    let prefix = &text[start..offset];

    ide_db::KNOWN_PRAGMAS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| {
            CompletionItem::new(CompletionItemKind::Keyword, format!("{}{name}", trigger as char))
                .detail("Sail pragma")
                .filter_text(*name)
                .insert_text(*name)
                // A pragma is only offered after its sigil, so it leads.
                .sort_text(format!("aaaa_{name}"))
        })
        .collect()
}
