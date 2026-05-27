//! Path finding for auto-import.
//! Given an item name and a "from" file, finds the `$include` path
//! needed to bring the item into scope. Used by:
//! - Auto-include completion (flyimport)
//! - "Add missing $include" quick-fix
//! - Import organization assists

use crate::workspace_def_map::WorkspaceDefMap;

/// Result of a path search: the `$include` path to add.
#[derive(Debug, Clone)]
pub struct ImportPath {
    pub include_path: String,
    pub defining_file: String,
}

/// Find the `$include` path to bring `item_name` into scope.
/// Returns `None` if not found or already in the current file.
pub fn find_path(
    workspace_def_map: &WorkspaceDefMap,
    item_name: &str,
    from_file_idx: usize,
) -> Option<ImportPath> {
    let defs = workspace_def_map.lookup(item_name);
    if defs.is_empty() {
        return None;
    }

    // Find the first definition NOT in the current file
    for def in defs {
        if def.file_index != from_file_idx {
            return Some(ImportPath {
                include_path: format!("file_{}", def.file_index),
                defining_file: format!("file_{}", def.file_index),
            });
        }
    }

    None // All definitions are in the current file — already in scope
}

/// All possible import paths for `item_name`, excluding `from_file_idx`.
pub fn find_all_paths(
    workspace_def_map: &WorkspaceDefMap,
    item_name: &str,
    from_file_idx: usize,
) -> Vec<ImportPath> {
    let defs = workspace_def_map.lookup(item_name);
    defs.iter()
        .filter(|def| def.file_index != from_file_idx)
        .map(|def| ImportPath {
            include_path: format!("file_{}", def.file_index),
            defining_file: format!("file_{}", def.file_index),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_path_returns_none_for_unknown() {
        let wdm = WorkspaceDefMap::build(&[]);
        assert!(find_path(&wdm, "nonexistent", 0).is_none());
    }
}
