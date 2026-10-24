//! Completion item rendering.
//!
//! Every completion item is built here, as rust-analyzer builds them in its
//! own `render` — the providers say *what* to offer and this says what it
//! looks like. It used to be `#[expect(dead_code)]` with no caller at all,
//! while each provider wrote the same nine-field literal by hand.
//!
//! Submodules:
//! - `pattern.rs`  — pattern completion rendering (match arms)

pub(crate) mod pattern;

use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;

/// A type name — struct, union, enum, alias, or a built-in.
pub(crate) fn render_type(name: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem::new(kind, name)
}

/// A field of a struct or bitfield.
///
/// Fields sort before everything else in the same list: inside a record
/// literal or after a `.`, they are what the cursor is asking about.
pub(crate) fn render_field(
    field_name: &str,
    field_type: Option<&str>,
    struct_name: &str,
) -> CompletionItem {
    let mut item = CompletionItem::new(CompletionItemKind::Field, field_name)
        .sort_text(format!("0{field_name}"))
        .documentation(format!("field of {struct_name}"));
    if let Some(ty) = field_type {
        item = item.detail(ty);
    }
    item
}

/// A field being named in a record literal, where the completion also
/// writes the `= `.
pub(crate) fn render_record_field(field_name: &str, struct_name: &str) -> CompletionItem {
    CompletionItem::new(CompletionItemKind::Field, field_name)
        .detail(format!("field of {struct_name}"))
        .insert_text(format!("{field_name} = "))
        .sort_text(format!("0{field_name}"))
}

/// A callable whose `val` spec exists but whose `function` does not, at
/// the site where that `function`'s name is being typed.
pub(crate) fn render_pending_definition(name: &str) -> CompletionItem {
    CompletionItem::new(CompletionItemKind::Function, name)
        .detail("declared by a val spec, not yet defined")
        .sort_text(format!("0{name}"))
}

/// A keyword that expands to a snippet — `match`, `if`, a `function` head.
pub(crate) fn render_keyword_snippet(keyword: &str, snippet: Option<String>) -> CompletionItem {
    CompletionItem::new(CompletionItemKind::Keyword, keyword)
        .detail("keyword")
        .insert_text(snippet.unwrap_or_else(|| keyword.to_owned()))
        .filter_text(keyword)
}

/// A code template, which sorts last.
pub(crate) fn render_snippet(label: &str, insert_text: &str, detail: &str) -> CompletionItem {
    CompletionItem::new(CompletionItemKind::Snippet, label)
        .detail(detail)
        .insert_text(insert_text)
        .sort_text(format!("zz{label}"))
}

// `render_keyword`, `render_variable`, `render_variant`, `render_function`
// and `render_struct_literal` were here too, and no provider wanted them:
// keywords arrive with a snippet, locals and enum members go through the
// same path as everything else in an expression, a call's snippet comes
// from `ide_db::function_snippet` — which is where the arity lives — and no
// provider offers a struct literal with its fields filled in. A render
// function with no provider is not a rendering layer.
