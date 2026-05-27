//! Snippet completion provider — code templates.

use super::Completions;
use crate::context::CompletionContext;

/// Complete code snippet templates (funcdecl, scattered, enumdef, etc.).
pub(crate) fn complete_snippet(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let items = crate::snippet_completions(ctx.prefix, ctx.is_top_level);
    acc.add_many(items);
}
