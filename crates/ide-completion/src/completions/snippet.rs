//! Snippet completion provider — code templates.

use super::Completions;
use crate::context::CompletionContext;
use crate::item::CompletionItem;

/// Complete code snippet templates (funcdecl, scattered, enumdef, etc.).
pub(crate) fn complete_snippet(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let items = snippet_completions(ctx.prefix, ctx.is_top_level);
    acc.add_many(items);
}

/// Built-in Sail code snippet templates.
pub(crate) fn snippet_completions(prefix: &str, is_top_level: bool) -> Vec<CompletionItem> {
    use crate::snippet::{SnippetScope, SAIL_SNIPPETS};

    let prefix_lower = prefix.to_ascii_lowercase();
    let scope = if is_top_level { SnippetScope::Item } else { SnippetScope::Expr };

    let mut items = Vec::new();
    for snip in SAIL_SNIPPETS {
        // Filter by scope: Item snippets at top-level, Expr snippets in expressions.
        // Type snippets are shown in both contexts.
        if snip.scope != scope && snip.scope != SnippetScope::Type {
            continue;
        }
        if !prefix_lower.is_empty() && !snip.prefix.starts_with(&prefix_lower) {
            continue;
        }
        items.push(
            crate::render::render_snippet(snip.prefix, snip.body, snip.description)
                .filter_text(snip.prefix.to_string()),
        );
    }
    items
}
