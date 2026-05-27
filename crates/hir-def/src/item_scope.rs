//! Per-scope definition container.
//! Holds all definitions visible in a particular scope (file or block).
//! Name lookups are namespace-aware (Types vs Values).

use rustc_hash::FxHashMap;

use crate::item_id::ModuleDefId;
use crate::name::Name;
use crate::nameres::FxIndexMap;
use crate::per_ns::{ItemInNs, Namespace, PerNs};
use crate::visibility::Visibility;

/// Namespace-aware container of definitions visible in a scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemScope {
    types: FxIndexMap<Name, ModuleDefId>,
    values: FxIndexMap<Name, Vec<ModuleDefId>>,
    declarations: Vec<ModuleDefId>,
    import_origins: FxHashMap<Name, vfs::FileId>,
}

impl ItemScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate all names with their namespace resolutions.
    pub fn entries(&self) -> impl Iterator<Item = (&Name, PerNs)> + '_ {
        let type_names = self.types.keys();
        let value_names = self.values.keys();
        let all_names: std::collections::BTreeSet<&Name> = type_names.chain(value_names).collect();
        all_names.into_iter().map(move |name| {
            let t = self
                .types
                .get(name)
                .copied()
                .map(|id| ItemInNs { def: id, vis: Visibility::Public });
            let v = self
                .values
                .get(name)
                .and_then(|v| v.first().copied())
                .map(|id| ItemInNs { def: id, vis: Visibility::Public });
            (name, PerNs { types: t, values: v })
        })
    }

    /// Look up a name, returning per-namespace results.
    pub fn get(&self, name: &Name) -> PerNs {
        let t =
            self.types.get(name).copied().map(|id| ItemInNs { def: id, vis: Visibility::Public });
        let v = self
            .values
            .get(name)
            .and_then(|v| v.first().copied())
            .map(|id| ItemInNs { def: id, vis: Visibility::Public });
        PerNs { types: t, values: v }
    }

    /// Get all value definitions for an overloaded name.
    pub fn get_values(&self, name: &Name) -> &[ModuleDefId] {
        self.values.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// All declarations in source order.
    pub fn declarations(&self) -> &[ModuleDefId] {
        &self.declarations
    }

    pub fn types_count(&self) -> usize {
        self.types.len()
    }

    pub fn values_count(&self) -> usize {
        self.values.len()
    }

    /// Record that `def` is declared in this scope.
    pub(crate) fn declare(&mut self, def: ModuleDefId) {
        self.declarations.push(def);
    }

    /// Push a resolution into the namespace maps.
    pub(crate) fn push_res(&mut self, name: Name, def_id: ModuleDefId, ns: Namespace) {
        match ns {
            Namespace::Types => {
                self.types.insert(name, def_id);
            }
            Namespace::Values => {
                self.values.entry(name).or_default().push(def_id);
            }
        }
    }

    /// Which `$include` file brought `name` into this scope, if any.
    ///
    /// Returns `None` for names declared locally (not imported).
    pub fn origin_of(&self, name: &Name) -> Option<vfs::FileId> {
        self.import_origins.get(name).copied()
    }
}
