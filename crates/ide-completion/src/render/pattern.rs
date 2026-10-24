//! Pattern completion rendering.
//! Renders completions for match arms and other pattern positions,
//! including enum variant destructuring patterns.

use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;

/// Render a pattern completion item (e.g., enum variant in match arm).
pub(crate) fn render_variant_pat(
    name: &str,
    detail: Option<&str>,
    insert_text: Option<&str>,
) -> CompletionItem {
    let mut item = CompletionItem::new(CompletionItemKind::EnumMember, name);
    if let Some(detail) = detail {
        item = item.detail(detail);
    }
    match insert_text {
        Some(text) => item.insert_text(text),
        None => item,
    }
}
