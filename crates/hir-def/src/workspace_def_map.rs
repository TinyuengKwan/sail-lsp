//! Workspace-wide definition map: flat `HashMap<Name, Vec<GlobalDef>>`.
//!
//! Aggregates per-file `DefMap`s for cross-file name resolution.

use std::collections::HashMap;

use crate::item_tree::{ItemKind, ItemTree};
use crate::name::Name;
use parser::Span;

/// A definition visible at workspace scope.
#[derive(Debug, Clone)]
pub struct GlobalDef {
    pub file_index: usize,
    pub kind: ItemKind,
    pub signature_text: String,
    pub span: Span,
    pub is_clause: bool,
    pub visibility: crate::visibility::RawVisibility,
}

/// Workspace-wide name -> definitions index.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDefMap {
    pub defs: HashMap<Name, Vec<GlobalDef>>,
}

impl WorkspaceDefMap {
    /// Build from a slice of (file_index, &ItemTree) pairs.
    pub fn build(files: &[(usize, &ItemTree)]) -> Self {
        let mut defs: HashMap<Name, Vec<GlobalDef>> = HashMap::new();
        for &(file_index, tree) in files {
            for &id in tree.top_level_items() {
                defs.entry(id.name(tree).clone()).or_default().push(GlobalDef {
                    file_index,
                    kind: id.item_kind(tree),
                    signature_text: id.signature(tree).to_owned(),
                    span: id.span(tree),
                    is_clause: id.is_clause(tree),
                    visibility: id.visibility(tree),
                });
            }
        }
        Self { defs }
    }

    /// Look up all definitions for a name across the workspace.
    ///
    /// Accepts both `&Name` and `&str` (via `Name: Borrow<str>`).
    pub fn lookup(&self, name: &str) -> &[GlobalDef] {
        self.defs.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Look up all definitions by `&Name` (preferred API).
    pub fn lookup_name(&self, name: &Name) -> &[GlobalDef] {
        self.defs.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if a name exists anywhere in the workspace.
    pub fn contains(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// Check if a name exists and is visible from the given file.
    ///
    /// `@private` items from other files are excluded.
    pub fn contains_visible_from(&self, name: &str, from_file: base_db::FileId) -> bool {
        self.defs.get(name).is_some_and(|defs| {
            defs.iter().any(|d| {
                let vis = crate::visibility::Visibility::resolve(
                    d.visibility,
                    base_db::FileId::from_raw(d.file_index as u32),
                );
                vis.is_visible_from(from_file)
            })
        })
    }

    /// Check if a Name exists anywhere in the workspace (preferred API).
    pub fn contains_name(&self, name: &Name) -> bool {
        self.defs.contains_key(name)
    }

    /// Find the "best" definition for a name: prefer val spec over
    /// function def, prefer non-clause over clause.
    pub fn best_definition(&self, name: &str) -> Option<&GlobalDef> {
        let defs = self.lookup(name);
        if defs.is_empty() {
            return None;
        }
        // Prefer val spec (has type signature)
        if let Some(val) = defs.iter().find(|d| d.kind == ItemKind::ValSpec) {
            return Some(val);
        }
        // Prefer non-clause definitions
        if let Some(def) = defs.iter().find(|d| !d.is_clause) {
            return Some(def);
        }
        defs.first()
    }

    /// All unique names in the workspace.
    pub fn all_names(&self) -> impl Iterator<Item = &Name> {
        self.defs.keys()
    }

    /// Total number of definitions across all files.
    pub fn total_defs(&self) -> usize {
        self.defs.values().map(|v| v.len()).sum()
    }

    /// Build filtered by `AnalysisScope`.
    pub fn build_for_scope(
        scope: &crate::analysis_scope::AnalysisScope,
        all_trees: &[(base_db::FileId, usize, &ItemTree)],
    ) -> Self {
        let filtered: Vec<(usize, &ItemTree)> = all_trees
            .iter()
            .filter(|(fid, _, _)| scope.contains(*fid))
            .map(|(_, idx, tree)| (*idx, *tree))
            .collect();
        Self::build(&filtered)
    }

    /// Build filtered by `.sail_project` boundary.
    pub fn build_for_project(
        files: &[(usize, &ItemTree)],
        project_files: &std::collections::HashSet<usize>,
    ) -> Self {
        let filtered: Vec<(usize, &ItemTree)> =
            files.iter().filter(|(idx, _)| project_files.contains(idx)).copied().collect();
        Self::build(&filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_tree(source: &str) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        ItemTree::build_from_cst(&root)
    }

    #[test]
    fn empty_workspace() {
        let wdm = WorkspaceDefMap::build(&[]);
        assert_eq!(wdm.total_defs(), 0);
        assert!(wdm.lookup("anything").is_empty());
    }

    #[test]
    fn single_file_single_def() {
        let tree = item_tree("function f() = 0\n");
        let wdm = WorkspaceDefMap::build(&[(0, &tree)]);
        assert_eq!(wdm.total_defs(), 1);
        assert_eq!(wdm.lookup("f").len(), 1);
        assert_eq!(wdm.lookup("f")[0].file_index, 0);
        assert_eq!(wdm.lookup("f")[0].kind, ItemKind::Function);
    }

    #[test]
    fn cross_file_val_spec_and_function() {
        let tree_a = item_tree("val add : int -> int\n");
        let tree_b = item_tree("function add(x) = x + 1\n");
        let wdm = WorkspaceDefMap::build(&[(0, &tree_a), (1, &tree_b)]);
        let defs = wdm.lookup("add");
        assert_eq!(defs.len(), 2);
        // best_definition prefers val spec
        let best = wdm.best_definition("add").unwrap();
        assert_eq!(best.kind, ItemKind::ValSpec);
        assert_eq!(best.file_index, 0);
    }

    #[test]
    fn scattered_across_files() {
        let tree_a = item_tree("scattered function foo\n");
        let tree_b = item_tree("function clause foo(0) = 0\n");
        let tree_c = item_tree("end foo\n");
        let wdm = WorkspaceDefMap::build(&[(0, &tree_a), (1, &tree_b), (2, &tree_c)]);
        let defs = wdm.lookup("foo");
        assert!(defs.len() >= 2, "should have head + clause + end");
    }

    #[test]
    fn best_definition_prefers_non_clause() {
        let tree = item_tree("function f() = 0\nfunction clause f(1) = 1\n");
        let wdm = WorkspaceDefMap::build(&[(0, &tree)]);
        let best = wdm.best_definition("f").unwrap();
        assert!(!best.is_clause);
    }

    #[test]
    fn all_names_iterates_unique_names() {
        let tree_a = item_tree("function f() = 0\nfunction g() = 1\n");
        let tree_b = item_tree("val f : int -> int\n");
        let wdm = WorkspaceDefMap::build(&[(0, &tree_a), (1, &tree_b)]);
        let mut names: Vec<&str> = wdm.all_names().map(|n| n.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["f", "g"]);
    }

    #[test]
    fn sail_riscv_cross_file_lookup() {
        // Simulate sail-riscv: prelude defines types, model files use them
        let prelude = item_tree("type xlenbits = bits(64)\nval pc_read : unit -> xlenbits\n");
        let model = item_tree("function step() = { let pc = pc_read(); pc }\n");
        let wdm = WorkspaceDefMap::build(&[(0, &prelude), (1, &model)]);

        // Lookup across files
        assert!(wdm.contains("xlenbits"), "type alias from prelude");
        assert!(wdm.contains("pc_read"), "val spec from prelude");
        assert!(wdm.contains("step"), "function from model");

        // best_definition prefers val spec
        let best = wdm.best_definition("pc_read").unwrap();
        assert_eq!(best.kind, ItemKind::ValSpec);
        assert_eq!(best.file_index, 0); // prelude file
    }
}
