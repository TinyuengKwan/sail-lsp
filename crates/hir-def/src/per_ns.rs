//! Per-namespace resolution result.
//!
//! is no macro namespace, but we keep the two-namespace (Types + Values)
//! model extensible.
//!
//! Each namespace slot carries an optional `(DefId, Visibility)` pair,
//! matching RA's `Item<Def, Import>` pattern where each entry carries
//! its visibility metadata.

use crate::item_id::DefId;
use crate::visibility::Visibility;

/// Which namespace a definition lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// Type aliases, struct, union, enum, bitfield, newtype.
    Types,
    /// Functions, val specs, registers, let bindings, enum members, constructors.
    Values,
}

/// A definition in a particular namespace, with its visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemInNs {
    pub def: DefId,
    pub vis: Visibility,
}

/// Combined per-namespace resolution.
///
/// `ItemInNs` with the definition and its visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerNs {
    pub types: Option<ItemInNs>,
    pub values: Option<ItemInNs>,
}

impl PerNs {
    pub fn none() -> Self {
        Self { types: None, values: None }
    }

    /// Type namespace entry with default (public) visibility.
    pub fn types(id: DefId) -> Self {
        Self { types: Some(ItemInNs { def: id, vis: Visibility::Public }), values: None }
    }

    /// Value namespace entry with default (public) visibility.
    pub fn values(id: DefId) -> Self {
        Self { types: None, values: Some(ItemInNs { def: id, vis: Visibility::Public }) }
    }

    /// Both namespaces with default (public) visibility.
    pub fn both(id: DefId) -> Self {
        Self {
            types: Some(ItemInNs { def: id, vis: Visibility::Public }),
            values: Some(ItemInNs { def: id, vis: Visibility::Public }),
        }
    }

    /// Type namespace entry with explicit visibility.
    pub fn types_vis(id: DefId, vis: Visibility) -> Self {
        Self { types: Some(ItemInNs { def: id, vis }), values: None }
    }

    /// Value namespace entry with explicit visibility.
    pub fn values_vis(id: DefId, vis: Visibility) -> Self {
        Self { types: None, values: Some(ItemInNs { def: id, vis }) }
    }

    /// Extract the DefId from the types namespace.
    pub fn take_types(self) -> Option<DefId> {
        self.types.map(|item| item.def)
    }

    /// Extract the DefId from the values namespace.
    pub fn take_values(self) -> Option<DefId> {
        self.values.map(|item| item.def)
    }

    pub fn is_none(&self) -> bool {
        self.types.is_none() && self.values.is_none()
    }

    pub fn or(self, other: PerNs) -> PerNs {
        PerNs { types: self.types.or(other.types), values: self.values.or(other.values) }
    }

    /// Filter entries by visibility.
    pub fn filter_visibility(self, f: impl Fn(&Visibility) -> bool) -> PerNs {
        PerNs {
            types: self.types.filter(|item| f(&item.vis)),
            values: self.values.filter(|item| f(&item.vis)),
        }
    }
}
