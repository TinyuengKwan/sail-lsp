//! Pragma completion provider — `@name` and `$name` directives.
//!
//! Completes known pragma names.

use super::Completions;
use crate::context::CompletionContext;

/// Complete pragma names after `@` or `$`.
pub(crate) fn complete_pragma(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let items = crate::pragma_completions(ctx.text, ctx.offset);
    acc.add_many(items);
}
