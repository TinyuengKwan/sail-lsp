//! Type position completion — suggests types after `:` or `->`.
//! Triggers when the cursor is in a type annotation context:
//! - After `:` in `let x : |`, `val f : |`, function params
//! - After `->` in return type position
//!
//! Suggests:
//! - Built-in types (int, nat, bool, string, unit, real, bits, etc.)
//! - User-defined types from the workspace (struct, enum, union, bitfield, type aliases)

use ide_db::defs::{CompletionItemKind, SymbolKind};
use ide_db::ide_types::{CompletionItem, CompletionRelevance};
use parser::Token;

#[allow(unused_imports)]
use ide_db::FileDb;

/// Built-in Sail types offered in type-annotation positions.
const BUILTIN_TYPES: &[(&str, &str)] = &[
    ("int", "arbitrary-precision integer"),
    ("nat", "non-negative integer (int where n >= 0)"),
    ("bool", "boolean"),
    ("bit", "single bit"),
    ("unit", "unit type ()"),
    ("string", "string"),
    ("real", "real number"),
    ("bits", "bitvector: bits('n)"),
    ("option", "option type: option('a)"),
    ("list", "list type: list('a)"),
    ("vector", "vector type: vector('n, 'a)"),
    ("range", "integer range: range('lo, 'hi)"),
    ("atom", "singleton integer: atom('n)"),
    ("register", "register reference: register('a)"),
];

/// Produce type-position completions.
/// # Arguments
/// - `prefix`: the text typed so far (for filtering)
/// - `all_type_names`: user-defined types from workspace
pub fn complete_type_pos(
    prefix: &str,
    all_type_names: &[(&str, SymbolKind)],
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // 1. Built-in types
    for &(name, detail) in BUILTIN_TYPES {
        if name.starts_with(prefix) || prefix.is_empty() {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: CompletionItemKind::Struct,
                detail: Some(detail.to_string()),
                insert_text: None,
                text_edit: None,
                sort_text: None,
                filter_text: None,
                documentation: None,
                deprecated: false,
                relevance: CompletionRelevance::default(),
            });
        }
    }

    // 2. User-defined types from workspace
    for &(name, kind) in all_type_names {
        if name.starts_with(prefix) || prefix.is_empty() {
            let ck = match kind {
                SymbolKind::Struct => CompletionItemKind::Struct,
                SymbolKind::Enum => CompletionItemKind::Enum,
                SymbolKind::TypeAlias => CompletionItemKind::Struct,
                _ => CompletionItemKind::Struct,
            };
            items.push(CompletionItem {
                label: name.to_string(),
                kind: ck,
                detail: None,
                insert_text: None,
                text_edit: None,
                sort_text: None,
                filter_text: None,
                documentation: None,
                deprecated: false,
                relevance: CompletionRelevance::default(),
            });
        }
    }

    items
}

/// Check if the cursor is in a type-annotation position.
///
/// Returns true if the token before the cursor suggests a type context:
/// - After `:` (type annotation)
/// - After `->` (return type)
/// - After `<->` (bidir mapping type)
#[allow(dead_code)] // WIP: used by type-position completion provider
pub fn is_type_position(prev_token: Option<&Token>) -> bool {
    match prev_token {
        Some(Token::Colon) => true,
        Some(Token::RightArrow) => true,  // ->
        Some(Token::DoubleArrow) => true, // <->
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_type_count() {
        assert_eq!(BUILTIN_TYPES.len(), 14);
    }

    #[test]
    fn is_type_pos_after_colon() {
        assert!(is_type_position(Some(&Token::Colon)));
    }

    #[test]
    fn is_type_pos_after_right_arrow() {
        assert!(is_type_position(Some(&Token::RightArrow)));
    }

    #[test]
    fn is_type_pos_after_double_arrow() {
        assert!(is_type_position(Some(&Token::DoubleArrow)));
    }

    #[test]
    fn is_type_pos_not_after_keyword() {
        assert!(!is_type_position(Some(&Token::KwFunction)));
    }

    #[test]
    fn is_type_pos_none() {
        assert!(!is_type_position(None));
    }
}
