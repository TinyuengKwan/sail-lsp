//! Per-file definition map.
//! The `DefMap` stores per-file module data with namespace-aware scopes.
//! Each file is represented as a single root module containing an
//! `ItemScope` (Sail has no nested modules).

pub(crate) mod collector;
pub mod diagnostics;
pub mod path_resolution;

use std::ops::{Deref, DerefMut, Index, IndexMut};
use std::sync::Arc;

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

use crate::item_scope::ItemScope;
use crate::item_tree::{ItemKind, ItemTree};
use crate::name::Name;

/// `FxIndexMap` — `IndexMap` with `FxBuildHasher`.
pub(crate) type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

/// Identifier for a module within a [`DefMap`].
///
/// Sail has no nested modules, so there is always exactly one module
/// per file (the root module, `ModuleId(0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

/// Where a module comes from. Sail only has file-level modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleOrigin {
    /// Module corresponds to a Sail source file.
    File,
}

/// Per-module data.
///
/// Simplified for Sail: no `parent`, no `children`
/// (Sail has no nested modules).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleData {
    /// Where this module comes from.
    pub origin: Option<ModuleOrigin>,
    /// The definitions visible in this module.
    pub scope: ItemScope,
}

/// A newtype wrapper around `FxIndexMap<ModuleId, ModuleData>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulesMap {
    inner: FxIndexMap<ModuleId, ModuleData>,
}

impl ModulesMap {
    fn new() -> Self {
        Self { inner: FxIndexMap::default() }
    }

    pub fn iter(&self) -> impl Iterator<Item = (ModuleId, &ModuleData)> + '_ {
        self.inner.iter().map(|(&k, v)| (k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ModuleId, &mut ModuleData)> + '_ {
        self.inner.iter_mut().map(|(&k, v)| (k, v))
    }

    pub fn insert(&mut self, id: ModuleId, data: ModuleData) {
        self.inner.insert(id, data);
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

impl Deref for ModulesMap {
    type Target = FxIndexMap<ModuleId, ModuleData>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ModulesMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Index<ModuleId> for ModulesMap {
    type Output = ModuleData;
    fn index(&self, id: ModuleId) -> &ModuleData {
        self.inner.get(&id).unwrap_or_else(|| panic!("ModuleId not found in ModulesMap: {id:?}"))
    }
}

impl IndexMut<ModuleId> for ModulesMap {
    fn index_mut(&mut self, id: ModuleId) -> &mut ModuleData {
        self.inner
            .get_mut(&id)
            .unwrap_or_else(|| panic!("ModuleId not found in ModulesMap: {id:?}"))
    }
}

/// Stable identifier for a top-level definition inside a single file.
/// Indexes into the parallel `data` Vec on [`DefMap`]; the index is
/// minted in `ItemTree` order so it's stable across body-only edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId(pub u32);

/// One row in the def map corresponding to a single `ItemTree` entry;
/// promotes a few fields for fast lookup.
#[derive(Debug, Clone)]
pub struct DefData {
    /// Definition name (as it appeared in source).
    pub name: Name,
    /// Item kind from the corresponding `ItemTree` entry.
    pub kind: ItemKind,
    /// Visibility of this definition.
    pub visibility: crate::visibility::RawVisibility,
    /// Index into the originating `ItemTree::entries` Vec.
    pub item_tree_index: usize,
}

/// Per-file definition map.
///
/// All name lookups delegate to `modules[root].scope` (the root
/// module's `ItemScope`).  The `data` vec provides `DefId -> DefData`
/// metadata access.
#[derive(Debug, Clone)]
pub struct DefMap {
    /// The root module of this file.
    pub root: ModuleId,
    /// Module storage.
    pub modules: ModulesMap,
    /// `DefId → DefData` mapping for metadata.
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
        let root = ModuleId(0);
        let mut modules = ModulesMap::new();
        modules.insert(root, ModuleData::default());
        Self {
            root,
            modules,
            data: Vec::new(),
            diagnostics: Vec::new(),
            prelude: default_prelude(),
        }
    }
}

impl DefMap {
    /// Build a [`DefMap`] from a per-file [`ItemTree`].
    ///
    /// Each item-tree entry yields one `DefId`; multiple entries with
    /// the same name (function clauses, scattered heads) are kept
    /// separately.
    pub fn build(item_tree: &Arc<ItemTree>) -> Self {
        use crate::per_ns::Namespace;

        let mut dm = Self::default();
        dm.modules[dm.root].origin = Some(ModuleOrigin::File);

        for (idx, &id) in item_tree.top_level_items().iter().enumerate() {
            let kind = id.item_kind(item_tree);
            let name = id.name(item_tree).clone();
            let visibility = id.visibility(item_tree);

            let raw_id = DefId(dm.data.len() as u32);
            dm.data.push(DefData { name: name.clone(), kind, visibility, item_tree_index: idx });

            // Convert raw DefId to typed ModuleDefId for namespace storage.
            let module_def_id = crate::item_id::ModuleDefId::from_def_id(raw_id, kind);

            // Declare() tracks what's in this scope,
            // push_res() makes it visible in namespace maps.
            let ns = match kind {
                ItemKind::Struct
                | ItemKind::Union
                | ItemKind::Enum
                | ItemKind::Bitfield
                | ItemKind::Newtype
                | ItemKind::TypeAlias => Namespace::Types,
                _ => Namespace::Values,
            };

            // Detect duplicate type definitions.
            if ns == Namespace::Types {
                let existing = dm.modules[dm.root].scope.get(&name);
                if let Some(first_item) = existing.types {
                    dm.diagnostics.push(DefDiagnostic::DuplicateDefinition {
                        name: name.clone(),
                        first: DefId(first_item.def.as_raw()),
                        second: raw_id,
                    });
                }
            }

            let scope = &mut dm.modules[dm.root].scope;
            scope.declare(module_def_id);
            scope.push_res(name, module_def_id, ns);
        }
        dm
    }

    /// The root module id.
    pub fn root_module_id(&self) -> ModuleId {
        self.root
    }

    /// Access the root module's data.
    pub fn root_module_data(&self) -> &ModuleData {
        &self.modules[self.root]
    }

    /// Access the root module's [`ItemScope`].
    ///
    /// Convenience — equivalent to `self[self.root].scope`.
    pub fn root_scope(&self) -> &ItemScope {
        &self.modules[self.root].scope
    }

    /// Mutable access to the root module's [`ItemScope`].
    pub(crate) fn root_scope_mut(&mut self) -> &mut ItemScope {
        &mut self.modules[self.root].scope
    }

    /// Iterate all modules.
    pub fn modules(&self) -> impl Iterator<Item = (ModuleId, &ModuleData)> + '_ {
        self.modules.iter()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn get(&self, id: DefId) -> Option<&DefData> {
        self.data.get(id.0 as usize)
    }

    /// All definitions, in `ItemTree` source order.
    pub fn entries(&self) -> &[DefData] {
        &self.data
    }

    /// All `ModuleDefId`s associated with `name` (values namespace).
    /// Multiple ids possible for function clauses, scattered heads, etc.
    pub fn lookup_name(&self, name: &str) -> &[crate::item_id::ModuleDefId] {
        let name_key = Name::from(name);
        self.root_scope().get_values(&name_key)
    }

    /// Get the `ItemKind` for a `DefId`.
    pub fn kind_of(&self, id: DefId) -> Option<ItemKind> {
        self.data.get(id.0 as usize).map(|d| d.kind)
    }

    /// Push a new DefData entry and return its DefId.
    /// Used by DefCollector to add definitions.
    pub(crate) fn alloc_def(
        &mut self,
        name: Name,
        kind: ItemKind,
        visibility: crate::visibility::RawVisibility,
        item_tree_index: usize,
    ) -> DefId {
        let id = DefId(self.data.len() as u32);
        self.data.push(DefData { name, kind, visibility, item_tree_index });
        id
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

    /// Push a diagnostic during construction.
    pub(crate) fn push_diagnostic(&mut self, diag: DefDiagnostic) {
        self.diagnostics.push(diag);
    }
}

/// Index DefMap by ModuleId to get ModuleData.
impl std::ops::Index<ModuleId> for DefMap {
    type Output = ModuleData;

    fn index(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id]
    }
}

impl std::ops::IndexMut<ModuleId> for DefMap {
    fn index_mut(&mut self, id: ModuleId) -> &mut ModuleData {
        &mut self.modules[id]
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
        assert_eq!(dm.get(DefId(ids[0].as_raw())).unwrap().name, "f");
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
    fn index_by_module_id() {
        let it = item_tree_for("function f() = 0\n");
        let dm = DefMap::build(&it);
        let root = dm.root_module_id();
        // Index trait works
        let module_data = &dm[root];
        assert!(module_data.origin.is_some());
        // root_scope() is same as indexing
        assert_eq!(dm.root_scope().types_count(), dm[root].scope.types_count());
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
