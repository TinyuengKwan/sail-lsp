//! File structure — document outline / breadcrumbs.
//! We use the same pattern for consistency.

use ide_db::ide_types::{NavigationTarget, SymbolKind};
use ide_db::line_index::TextRange;
use ide_db::FileDb;

/// A node in the file structure outline.
#[derive(Debug, Clone)]
pub struct StructureNode {
    /// Index of the parent node in the flat list (None for top-level).
    pub parent: Option<usize>,
    /// Display label (symbol name).
    pub label: String,
    /// Range of the symbol name (for cursor placement).
    pub navigation_range: TextRange,
    /// Range of the entire definition (for highlighting).
    pub node_range: TextRange,
    /// Symbol kind.
    pub kind: StructureNodeKind,
    /// Optional detail text (e.g., type signature).
    pub detail: Option<String>,
    /// Whether the symbol is deprecated.
    pub deprecated: bool,
}

/// Kind of structure node.
/// Wraps SymbolKind + special kinds (Region, ExternBlock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StructureNodeKind {
    SymbolKind(SymbolKind),
    Region,
}

/// Configuration for file structure computation.
#[derive(Debug, Clone, Default)]
pub struct FileStructureConfig {
    /// Whether to include local bindings (let/var inside functions).
    pub include_locals: bool,
}

/// Compute the file structure (document outline).
pub fn file_structure(file: &dyn FileDb) -> Vec<StructureNode> {
    file_structure_with_config(file, &FileStructureConfig::default())
}

/// Compute file structure with configuration.
pub fn file_structure_with_config(
    file: &dyn FileDb,
    _config: &FileStructureConfig,
) -> Vec<StructureNode> {
    // Delegate to existing document_symbols_ide which builds NavigationTarget tree
    let nav_targets = ide_db::symbol_index::document_symbols_ide(file);
    let mut result = Vec::new();
    flatten_nav_targets(&nav_targets, None, &mut result);
    result
}

/// Convert NavigationTarget tree to flat StructureNode list with parent indices.
fn flatten_nav_targets(
    targets: &[NavigationTarget],
    parent: Option<usize>,
    result: &mut Vec<StructureNode>,
) {
    for target in targets {
        let idx = result.len();
        result.push(StructureNode {
            parent,
            label: target.name.clone(),
            navigation_range: target.focus_range,
            node_range: target.full_range,
            kind: StructureNodeKind::SymbolKind(target.kind),
            detail: target.detail.clone(),
            deprecated: false,
        });
        // Recurse into children
        if !target.children.is_empty() {
            flatten_nav_targets(&target.children, Some(idx), result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_node_kind_symbol() {
        let kind = StructureNodeKind::SymbolKind(SymbolKind::Function);
        assert_eq!(kind, StructureNodeKind::SymbolKind(SymbolKind::Function));
    }
}
