//! Mapping between hir IDs and the surface syntax.
//!
//! `HasSource` retrieves a definition's source location as `InFile<SyntaxNodePtr>`.

use syntax::SyntaxNodePtr;

use crate::in_file::InFile;
use crate::item_tree::ItemTree;
use crate::nameres::{DefData, DefId, DefMap};

/// HIR objects that have a source location.
pub trait HasSource {
    /// Source location of this definition.
    fn source(&self, item_tree: &ItemTree) -> Option<InFile<SyntaxNodePtr>>;
}

/// Resolve a `DefId` to its source `SyntaxNodePtr`.
pub fn def_source(
    file_id: base_db::FileId,
    def_data: &DefData,
    item_tree: &ItemTree,
) -> InFile<SyntaxNodePtr> {
    let span = item_tree.top_level_items()[def_data.item_tree_index].span(item_tree);
    let ptr = SyntaxNodePtr::from_range(
        parser::SyntaxKind::CALLABLE_DEF, // kind is approximate; range is exact
        rowan::TextRange::new(
            rowan::TextSize::from(span.start as u32),
            rowan::TextSize::from(span.end as u32),
        ),
    );
    InFile::new(file_id, ptr)
}

/// Look up the source for a `DefId` given a DefMap and ItemTree.
///
/// Convenience function combining `DefMap::get` + `def_source`.
pub fn def_id_source(
    file_id: base_db::FileId,
    def_id: DefId,
    def_map: &DefMap,
    item_tree: &ItemTree,
) -> Option<InFile<SyntaxNodePtr>> {
    let def_data = def_map.get(def_id)?;
    Some(def_source(file_id, def_data, item_tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::ItemTree;
    use base_db::FileId;
    use std::sync::Arc;

    #[test]
    fn def_source_returns_correct_span() {
        let source = "val add : (int, int) -> int\nfunction add(x, y) = x + y\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let def_map = DefMap::build(&Arc::new(tree.clone()));

        // Should have at least one definition
        assert!(!def_map.is_empty());

        let def_data = def_map.get(DefId(0)).unwrap();
        let result = def_source(FileId::from_raw(1), def_data, &tree);

        assert_eq!(result.file_id, FileId::from_raw(1));
        // The ptr should have a non-empty range
        assert!(result.value.text_range().len() > rowan::TextSize::from(0));
    }

    #[test]
    fn def_id_source_returns_none_for_invalid_id() {
        let source = "function f(x) = x\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let def_map = DefMap::build(&Arc::new(tree.clone()));

        // DefId(999) doesn't exist
        let result = def_id_source(FileId::from_raw(1), DefId(999), &def_map, &tree);
        assert!(result.is_none());
    }

    #[test]
    fn def_source_each_item_has_distinct_range() {
        let source = "function f(x) = x\nfunction g(y) = y\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let def_map = DefMap::build(&Arc::new(tree.clone()));

        assert!(def_map.len() >= 2);

        let src0 = def_source(FileId::from_raw(1), def_map.get(DefId(0)).unwrap(), &tree);
        let src1 = def_source(FileId::from_raw(1), def_map.get(DefId(1)).unwrap(), &tree);

        // Different definitions should have different ranges
        assert_ne!(src0.value.text_range(), src1.value.text_range());
    }
}
