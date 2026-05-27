//! Completion item rendering.
//! Provides functions to render different kinds of completion items
//! with appropriate labels, detail text, and insert text.
//!
//! Submodules.
//! - `function.rs` — function signature rendering
//! - `pattern.rs`  — pattern completion rendering (match arms)
//! - `literal.rs`  — literal/struct-literal rendering

pub(crate) mod function;
pub(crate) mod literal;
pub(crate) mod pattern;

use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::{CompletionItem, CompletionRelevance};

// Re-export primary render functions for backward compat.

/// Render a type completion item.
pub(crate) fn render_type(name: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind,
        detail: None,
        insert_text: None,
        text_edit: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}

/// Render a field completion item.
pub(crate) fn render_field(
    field_name: &str,
    field_type: Option<&str>,
    struct_name: &str,
) -> CompletionItem {
    CompletionItem {
        label: field_name.to_string(),
        kind: CompletionItemKind::Field,
        detail: field_type.map(|t| t.to_string()),
        insert_text: None,
        text_edit: None,
        sort_text: Some(format!("0{}", field_name)),
        filter_text: None,
        documentation: Some(format!("field of {}", struct_name)),
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}

/// Render a keyword completion item.
pub(crate) fn render_keyword(keyword: &str) -> CompletionItem {
    CompletionItem {
        label: keyword.to_string(),
        kind: CompletionItemKind::Keyword,
        detail: None,
        insert_text: None,
        text_edit: None,
        sort_text: Some(format!("z{}", keyword)), // keywords sort after identifiers
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}

/// Render a variable/local binding completion item.
pub(crate) fn render_variable(name: &str, ty: Option<&str>) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: CompletionItemKind::Variable,
        detail: ty.map(|t| t.to_string()),
        insert_text: None,
        text_edit: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}

/// Render a snippet completion item.
pub(crate) fn render_snippet(label: &str, insert_text: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: CompletionItemKind::Snippet,
        detail: Some(detail.to_string()),
        insert_text: Some(insert_text.to_string()),
        text_edit: None,
        sort_text: Some(format!("zz{}", label)), // snippets sort last
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}
