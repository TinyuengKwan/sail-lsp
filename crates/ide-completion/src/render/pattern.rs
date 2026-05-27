//! Pattern completion rendering.
//! Renders completions for match arms and other pattern positions,
//! including enum variant destructuring patterns.

use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::{CompletionItem, CompletionRelevance};

/// Render a pattern completion item (e.g., enum variant in match arm).
pub(crate) fn render_variant_pat(
    name: &str,
    detail: Option<&str>,
    insert_text: Option<&str>,
) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: CompletionItemKind::EnumMember,
        detail: detail.map(|d| d.to_string()),
        insert_text: insert_text.map(|t| t.to_string()),
        text_edit: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}
