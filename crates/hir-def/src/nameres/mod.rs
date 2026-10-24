//! Per-file definition map.
//! The `DefMap` stores per-file module data with namespace-aware scopes.
//! Each file is represented as a single root module containing an
//! `ItemScope` (Sail has no nested modules).

pub mod diagnostics;
pub mod path_resolution;

use std::sync::Arc;

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

use crate::item_scope::ItemScope;
use crate::item_tree::{DefKind, ItemTree};
use crate::name::Name;

/// `FxIndexMap` — `IndexMap` with `FxBuildHasher`.
pub(crate) type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

/// Stable identifier for a top-level definition inside a single file.
/// Indexes into the parallel `data` Vec on [`DefMap`]; the index is
/// minted in `ItemTree` order so it's stable across body-only edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefIdx(pub u32);

/// One row in the def map corresponding to a single `ItemTree` entry;
/// promotes a few fields for fast lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefData {
    /// Definition name (as it appeared in source).
    pub name: Name,
    /// Item kind from the corresponding `ItemTree` entry.
    pub kind: DefKind,
    /// Visibility of this definition.
    pub visibility: crate::visibility::RawVisibility,
    /// Index into the originating `ItemTree::entries` Vec.
    pub item_tree_index: usize,
}

/// Per-file definition map.
///
/// All name lookups delegate to `scope`. The `data` vec provides
/// `DefIdx -> DefData` metadata access.
///
/// There is one scope, not a tree of them: rust-analyzer's `DefMap` maps
/// `ModuleId -> ModuleData` because Rust nests modules inside a file, and
/// Sail does not. What Sail calls a module lives in a `.sail_project`
/// file and groups whole files; `project-model` handles that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefMap {
    /// Everything this file declares.
    scope: ItemScope,
    /// `DefIdx → DefData` mapping for metadata.
    data: Vec<DefData>,
    /// Diagnostics accumulated during DefMap construction.
    diagnostics: Vec<DefDiagnostic>,
    /// Built-in type names resolved as fallback when a name is not in module scope.
    prelude: Vec<Name>,
}

pub use diagnostics::DefDiagnostic;

/// The default set of Sail built-in type names (prelude).
fn default_prelude() -> Vec<Name> {
    [
        "int",
        "nat",
        "bool",
        "unit",
        "string",
        "real",
        "bit",
        "bits",
        "vector",
        "list",
        "option",
        "result",
        "range",
        "atom",
        "atom_bool",
        "implicit",
    ]
    .iter()
    .map(|&s| Name::from(s))
    .collect()
}

impl Default for DefMap {
    fn default() -> Self {
        Self {
            scope: ItemScope::default(),
            data: Vec::new(),
            diagnostics: Vec::new(),
            prelude: default_prelude(),
        }
    }
}

impl DefMap {
    /// Build a [`DefMap`] from a per-file [`ItemTree`].
    ///
    /// Each item-tree entry yields one `DefIdx`; multiple entries with
    /// the same name (function clauses, scattered heads) are kept
    /// separately.
    pub fn build(item_tree: &Arc<ItemTree>) -> Self {
        use crate::per_ns::Namespace;

        let mut dm = Self::default();

        for (idx, &id) in item_tree.top_level_items().iter().enumerate() {
            let kind = id.def_kind(item_tree);
            let name = id.name(item_tree).clone();
            let visibility = id.visibility(item_tree);

            let raw_id = DefIdx(dm.data.len() as u32);
            dm.data.push(DefData { name: name.clone(), kind, visibility, item_tree_index: idx });

            // Convert raw DefIdx to typed DefId for namespace storage.
            let module_def_id = crate::item_id::DefId::from_def_idx(raw_id, kind);

            // Declare() tracks what's in this scope,
            // push_res() makes it visible in namespace maps.
            let ns = match kind {
                DefKind::Struct
                | DefKind::Union
                | DefKind::Enum
                | DefKind::Bitfield
                | DefKind::Newtype
                | DefKind::TypeAlias => Namespace::Types,
                _ => Namespace::Values,
            };

            // Detect duplicate type definitions.
            if ns == Namespace::Types {
                let existing = dm.scope.get(&name);
                if let Some(first_item) = existing.types {
                    dm.diagnostics.push(DefDiagnostic::DuplicateDefinition {
                        name: name.clone(),
                        first: DefIdx(first_item.def.as_raw()),
                        second: raw_id,
                        kind,
                    });
                }
            }

            let scope = &mut dm.scope;
            scope.declare(module_def_id);
            scope.push_res(name, module_def_id, ns);
        }
        dm
    }

    /// Everything this file declares.
    pub fn root_scope(&self) -> &ItemScope {
        &self.scope
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn get(&self, id: DefIdx) -> Option<&DefData> {
        self.data.get(id.0 as usize)
    }

    /// All definitions, in `ItemTree` source order.
    pub fn entries(&self) -> &[DefData] {
        &self.data
    }

    /// All `DefId`s associated with `name` (values namespace).
    /// Multiple ids possible for function clauses, scattered heads, etc.
    pub fn lookup_name(&self, name: &str) -> &[crate::item_id::DefId] {
        let name_key = Name::from(name);
        self.root_scope().get_values(&name_key)
    }

    /// Get the `DefKind` for a `DefIdx`.
    pub fn kind_of(&self, id: DefIdx) -> Option<DefKind> {
        self.data.get(id.0 as usize).map(|d| d.kind)
    }
    /// Check if `name` is a built-in type available via the prelude.
    ///
    /// name is a Sail built-in type that should be resolvable without
    /// explicit `$include`.
    pub fn resolve_in_prelude(&self, name: &str) -> bool {
        self.prelude.iter().any(|n| n.as_str() == name)
    }

    /// The prelude names (built-in types).
    pub fn prelude(&self) -> &[Name] {
        &self.prelude
    }

    /// Diagnostics produced during DefMap construction.
    pub fn diagnostics(&self) -> &[DefDiagnostic] {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item_tree_for(source: &str) -> Arc<ItemTree> {
        let (root, _) = syntax::parse_text(source);
        Arc::new(ItemTree::build_from_cst(&root))
    }

    #[test]
    fn empty_file_has_empty_def_map() {
        let it = item_tree_for("");
        let dm = DefMap::build(&it);
        assert!(dm.is_empty());
        assert_eq!(dm.lookup_name("anything").len(), 0);
    }

    #[test]
    fn single_function_yields_one_def() {
        let it = item_tree_for("function f() = 0\n");
        let dm = DefMap::build(&it);
        assert_eq!(dm.len(), 1);
        let ids = dm.lookup_name("f");
        assert_eq!(ids.len(), 1);
        assert_eq!(dm.get(DefIdx(ids[0].as_raw())).unwrap().name, "f");
    }

    #[test]
    fn function_clauses_share_a_name_key() {
        let it = item_tree_for(
            "\
val pick : int -> int
function clause pick(0) = 100
function clause pick(_) = 0
",
        );
        let dm = DefMap::build(&it);
        let ids = dm.lookup_name("pick");
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn distinct_definitions_keep_distinct_ids() {
        let it = item_tree_for("function a() = 0\nfunction b() = 0\n");
        let dm = DefMap::build(&it);
        let a = dm.lookup_name("a");
        let b = dm.lookup_name("b");
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_ne!(a[0], b[0]);
    }

    #[test]
    fn a_file_has_one_scope() {
        let it = item_tree_for("function f() = 0\n");
        let dm = DefMap::build(&it);
        assert_eq!(dm.root_scope().types_count(), 0);
        assert_eq!(dm.len(), 1);
    }

    #[test]
    fn duplicate_type_emits_diagnostic() {
        // Two enum definitions with the same name → DuplicateDefinition diagnostic
        let it = item_tree_for("enum Foo = { A, B }\nenum Foo = { C, D }\n");
        let dm = DefMap::build(&it);
        let diags = dm.diagnostics();
        assert!(
            diags.iter().any(|d| matches!(d, DefDiagnostic::DuplicateDefinition { name, .. } if name.as_str() == "Foo")),
            "expected DuplicateDefinition for Foo, got: {:?}", diags
        );
    }

    #[test]
    fn function_clauses_no_duplicate_diagnostic() {
        // Function clauses share names → no diagnostic (only types trigger it)
        let it = item_tree_for("function clause f(0) = 1\nfunction clause f(_) = 0\n");
        let dm = DefMap::build(&it);
        let diags = dm.diagnostics();
        assert!(
            diags.is_empty(),
            "function clauses should NOT trigger duplicate diagnostic, got: {:?}",
            diags
        );
    }
}
