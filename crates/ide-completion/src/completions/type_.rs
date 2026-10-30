//! Type position completion — suggests types wherever the tree puts one
//! (see `context::in_type`), or after a bare `:` or `->`.
//!
//! Suggests:
//! - Built-in types (int, nat, bool, string, unit, real, bits, etc.)
//! - User-defined types from the workspace (struct, enum, union, bitfield, type aliases)

use crate::item::CompletionItem;
use ide_db::defs::{CompletionItemKind, SymbolKind};

#[allow(unused_imports)]
use ide_db::FileDb;
use url::Url;

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
            items.push(crate::render::render_type(name, CompletionItemKind::Struct).detail(detail));
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
            items.push(crate::render::render_type(name, ck));
        }
    }

    items
}

/// Collect type names from all workspace files.
pub(crate) fn collect_type_names(
    ctx: &crate::context::CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
) -> Vec<(String, ide_db::defs::SymbolKind)> {
    let mut type_names = Vec::new();
    for (_, f) in all_files {
        let Some(tree) = f.item_tree() else { continue };
        for &id in tree.top_level_items() {
            // Prefix rejection first: it is a comparison, and visibility is
            // a resolution per item. Asking visibility of every type in the
            // workspace cost 66 ms per type-position query on sail-riscv.
            if !id.name(tree).as_str().starts_with(ctx.prefix) {
                continue;
            }
            if !ctx.can_see_item(*f, tree, id) {
                continue;
            }
            use hir::DefKind as D;
            let kind = match id.def_kind(tree) {
                D::ScatteredHead if id.scattered_type_kind(tree).is_some() => {
                    ide_db::defs::SymbolKind::Enum
                }
                D::Struct | D::Bitfield => ide_db::defs::SymbolKind::Struct,
                D::Enum | D::Union => ide_db::defs::SymbolKind::Enum,
                // `newtype` names a type too, and the scrape's list left it
                // out.
                D::TypeAlias | D::Newtype => ide_db::defs::SymbolKind::TypeAlias,
                _ => continue,
            };
            type_names.push((id.name(tree).to_string(), kind));
        }
    }
    type_names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_type_count() {
        assert_eq!(BUILTIN_TYPES.len(), 14);
    }
}
