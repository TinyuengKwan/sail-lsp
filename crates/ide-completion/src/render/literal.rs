//! Literal value completion rendering.
//! Renders completions for literal values like struct literals,
//! enum variant constructors with fields, and boolean literals.

use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::{CompletionItem, CompletionRelevance};

/// Render a struct literal completion (e.g., `Foo { field1: _, field2: _ }`).
pub(crate) fn render_struct_literal(
    name: &str,
    fields_snippet: &str,
    detail: Option<&str>,
) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: CompletionItemKind::Struct,
        detail: detail.map(|d| d.to_string()),
        insert_text: Some(fields_snippet.to_string()),
        text_edit: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}
