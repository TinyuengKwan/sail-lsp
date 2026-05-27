//! Item list completion — top-level and block-level items.
//! RA entry: `complete_item_list(acc, ctx, path_ctx, kind)`
//! dispatches on `ItemListKind::SourceFile` vs `Module` vs `Impl` etc.
//!
//! Sail: top-level keywords (function, val, type, ...) and block-level
//! keywords (let, var, if, ...).

use super::Completions;

use crate::context::{CompletionContext, ItemListKind, PathCompletionCtx};

/// Complete items in item-list position.
pub(crate) fn complete_item_list(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    _path_ctx: &PathCompletionCtx,
    kind: &ItemListKind,
    keywords: &[&str],
) {
    match kind {
        ItemListKind::SourceFile => {
            // Top-level items: function, val, type, register, etc.
            super::keyword::complete_keywords(acc, ctx, keywords);
        }
        ItemListKind::Block => {
            // Block-level items: let, var, if, match, etc.
            super::keyword::complete_keywords(acc, ctx, keywords);
        }
    }
}
