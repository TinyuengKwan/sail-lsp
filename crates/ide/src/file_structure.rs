//! File structure — the document outline.
//!
//! rust-analyzer's placement: `ide/src/file_structure.rs` builds what
//! `textDocument/documentSymbol` answers, and `ide-db/src/symbol_index.rs`
//! is the *workspace* index. This module used to be 92 lines flattening
//! `ide_db::signatures::document_symbols_ide` into a parent-indexed list
//! that nothing consumed — a second representation of an outline the
//! protocol wants as a tree — while the builder itself sat one crate down.

use std::collections::HashMap;

use crate::navigation_target::NavigationTarget;
use ide_db::FileDb;

/// The outline `textDocument/documentSymbol` answers with.
///
/// Enum members become children of their parent enum; every other
/// top-level declaration is a root. Each symbol carries its precise
/// signature text from the `ItemTree` as `detail`, rather than the bare
/// category label.
pub fn file_structure(file: &dyn FileDb) -> Vec<NavigationTarget> {
    use ide_db::ide_types::SymbolKind as IdeSymbolKind;

    let Some(tree) = file.item_tree() else {
        return Vec::new();
    };

    let placeholder = url::Url::parse("file:///").expect("a literal URL parses");
    let target = |name: String,
                  kind: IdeSymbolKind,
                  detail: String,
                  full: base_db::TextRange,
                  focus: base_db::TextRange| {
        // Single-file outline: the caller knows which file it asked about,
        // so the URL is a placeholder.
        NavigationTarget::new(placeholder.clone(), name, kind, full, focus).with_detail(detail)
    };

    let mut roots: Vec<NavigationTarget> = Vec::new();
    for &id in tree.top_level_items() {
        let kind = match id.def_kind(tree) {
            hir::DefKind::Function
            | hir::DefKind::ValSpec
            | hir::DefKind::Mapping
            | hir::DefKind::MappingSpec
            | hir::DefKind::Overload
            | hir::DefKind::Outcome
            | hir::DefKind::ScatteredHead
            | hir::DefKind::ScatteredClause => IdeSymbolKind::Function,
            hir::DefKind::Register | hir::DefKind::Let | hir::DefKind::Var => {
                IdeSymbolKind::Variable
            }
            hir::DefKind::TypeAlias
            | hir::DefKind::Struct
            | hir::DefKind::Union
            | hir::DefKind::Bitfield
            | hir::DefKind::Newtype => IdeSymbolKind::Struct,
            hir::DefKind::Enum => IdeSymbolKind::Enum,
            _ => continue,
        };
        let span = id.span(tree);
        let name_span = id.name_span(tree);
        let mut root = target(
            id.name(tree).to_string(),
            kind,
            id.signature(tree).to_string(),
            base_db::text_range(span.start, span.end),
            base_db::text_range(name_span.start, name_span.end),
        );
        // Members belong to the definition that declares them. Parenting
        // used to be decided by span containment, with "the most recently
        // declared enum" as a fallback when that failed.
        if matches!(id.def_kind(tree), hir::DefKind::Enum | hir::DefKind::Union) {
            for field in id.fields(tree) {
                let r = base_db::text_range(field.name_span.start, field.name_span.end);
                root.children.push(target(
                    field.name.to_string(),
                    IdeSymbolKind::EnumMember,
                    "enum member".to_string(),
                    r,
                    r,
                ));
            }
        }
        roots.push(root);
    }

    // Nest bitfield synthetic accessors under their parent bitfield.
    // ItemTree generates _get_/update_/set_ entries for each bitfield field.
    // Group them as children of the bitfield entry in the outline.
    let mut bitfield_indices: HashMap<String, usize> = HashMap::new();
    for (idx, target) in roots.iter().enumerate() {
        if target.kind == IdeSymbolKind::Struct
            && target.detail.as_deref().is_some_and(|d| d.starts_with("bitfield"))
        {
            bitfield_indices.insert(target.name.clone(), idx);
        }
    }
    // Move synthetic bitfield accessors into their parent's children
    let mut to_remove = Vec::new();
    for (idx, target) in roots.iter().enumerate() {
        for (bf_name, &parent_idx) in &bitfield_indices {
            if target.name.contains(&format!("_{bf_name}_"))
                && (target.name.starts_with("_get_")
                    || target.name.starts_with("_update_")
                    || target.name.starts_with("_set_")
                    || target.name.starts_with("Mk_"))
            {
                to_remove.push((idx, parent_idx));
                break;
            }
        }
    }
    // Apply nesting (reverse order to preserve indices)
    for &(child_idx, parent_idx) in to_remove.iter().rev() {
        if child_idx < roots.len() {
            let child = roots.remove(child_idx);
            // Adjust parent_idx if it was after the removed child
            let adjusted_parent = if parent_idx > child_idx { parent_idx - 1 } else { parent_idx };
            if let Some(parent) = roots.get_mut(adjusted_parent) {
                parent.children.push(child);
            }
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use crate::fixture;

    /// Through `Analysis::document_symbols`, the one door.
    ///
    /// The builder used to live in `ide_db::symbol_index` — the *workspace*
    /// index — with this module a flat adapter over it that nothing called.
    #[test]
    fn the_outline_names_what_the_file_declares() {
        let source = "\
val add : (int, int) -> int
function add(x, y) = x + y
enum E = {A, B}
";
        let (analysis, files) = fixture::analysis(source);
        let symbols = analysis.document_symbols(files[0].0).expect("no cancellation");

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "missing `add`: {names:?}");
        assert!(names.contains(&"E"), "missing `E`: {names:?}");

        // A signature, not the bare category label.
        let add = symbols.iter().find(|s| s.name == "add").expect("add");
        assert!(add.detail.is_some(), "no detail on `add`: {add:?}");

        // Enum members are children of their enum, not roots.
        let e = symbols.iter().find(|s| s.name == "E").expect("E");
        let members: Vec<&str> = e.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(members, vec!["A", "B"], "enum members are not its children");
    }
}
