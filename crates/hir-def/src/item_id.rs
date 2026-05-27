//! Typed item identifiers (`FunctionId`, `TypeDefId`, etc.) and
//! location types (`FunctionLoc`, etc.) recording where items are defined.

use base_db::FileId;
use la_arena::Idx;

use crate::item_tree;
use crate::nameres::DefId;

//
// Provides the canonical API for creating IDs from locations and
// recovering locations from IDs.
//
// Current implementation is a no-op passthrough (raw index stored in
// both Loc and Id). When IDs become `#[salsa::interned]`, these impls
// will delegate to the salsa database.

/// Intern a location to obtain its ID.
pub trait Intern {
    type ID: Copy;
    fn intern(self, db: &dyn crate::db::DefDatabase) -> Self::ID;
}

/// Look up the location data for an ID.
pub trait Lookup {
    type Data;
    fn lookup(self, db: &dyn crate::db::DefDatabase) -> Self::Data;
}

/// Identifier for a function definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

/// Identifier for a type definition (struct/union/enum/bitfield/newtype/alias).
///
/// distinguish ADT from type alias at the ID level).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeDefId(pub u32);

/// Identifier for a register definition.
///
/// Sail-specific (no RA counterpart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegisterId(pub u32);

/// Identifier for a val specification.
///
/// Sail-specific — RA has no separate "val spec" concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValSpecId(pub u32);

/// Identifier for a mapping definition.
///
/// Sail-specific (bidirectional encoding/decoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MappingId(pub u32);

/// Identifier for a top-level let/var binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LetId(pub u32);

/// Identifier for an overload declaration.
///
/// Sail-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OverloadId(pub u32);

//
// Each Location records where an item is defined: which file and
// which index in the ItemTree's typed arena.
//
// ```
// pub struct ItemLoc<N> {
//     pub container: ModuleId,
//     pub id: AstId<N>,
// }
// ```
// Sail version uses `FileId + Idx<T>` since Sail has no nested modules.

/// Location of a function in the source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::Function>,
}

/// Location of a type definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeDefLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::TypeDef>,
}

/// Location of a register.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegisterLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::Register>,
}

/// Location of a val specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValSpecLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::ValSpec>,
}

/// Location of a mapping definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MappingLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::Mapping>,
}

/// Location of a top-level let/var binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LetLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::LetDef>,
}

/// Location of an overload declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OverloadLoc {
    pub file_id: FileId,
    pub item_tree_idx: Idx<item_tree::Overload>,
}

//
// Current implementation: extract raw index from `item_tree_idx`.
// Future: delegate to `#[salsa::interned]` which stores the full Loc.

macro_rules! impl_intern {
    ($loc:ty, $id:ty, $field_ty:ty) => {
        impl Intern for $loc {
            type ID = $id;
            fn intern(self, _db: &dyn crate::db::DefDatabase) -> $id {
                <$id>::from_raw(u32::from(self.item_tree_idx.into_raw()))
            }
        }
    };
}

impl FunctionId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl TypeDefId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl RegisterId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl ValSpecId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl MappingId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl LetId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}
impl OverloadId {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn into_raw(self) -> u32 {
        self.0
    }
}

impl_intern!(FunctionLoc, FunctionId, item_tree::Function);
impl_intern!(TypeDefLoc, TypeDefId, item_tree::TypeDef);
impl_intern!(RegisterLoc, RegisterId, item_tree::Register);
impl_intern!(ValSpecLoc, ValSpecId, item_tree::ValSpec);
impl_intern!(MappingLoc, MappingId, item_tree::Mapping);
impl_intern!(LetLoc, LetId, item_tree::LetDef);
impl_intern!(OverloadLoc, OverloadId, item_tree::Overload);

//
// NOTE: Lookup cannot be implemented without a backing store that maps
// Id → Loc. Once IDs become `#[salsa::interned]`, salsa provides this
// automatically via the generated `data()` method. For now, lookup is
// performed through the DefMap (DefId → item info).
//
// The Lookup trait is defined above but impls are deferred to the salsa
// migration. Call sites that need Id→Loc today should use DefMap queries.

/// All definitions visible in module scope.
/// This is the typed alternative to the raw `DefId`. Consumers will
/// gradually migrate from `DefId` to `ModuleDefId`.
///
/// Variants mirror RA's `TraitId`/`AdtId`/`EnumVariantId`/`MacroId`/`ImplId`
/// as plain u32 newtypes (not salsa-interned keys with Loc data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleDefId {
    FunctionId(FunctionId),
    TypeDefId(TypeDefId),
    RegisterId(RegisterId),
    ValSpecId(ValSpecId),
    MappingId(MappingId),
    LetId(LetId),
    OverloadId(OverloadId),
}

/// Definitions that have bodies (function bodies, mapping clauses).
/// These are the targets of per-callable type inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefWithBodyId {
    FunctionId(FunctionId),
    MappingId(MappingId),
    LetId(LetId),
}

impl From<FunctionId> for ModuleDefId {
    fn from(id: FunctionId) -> Self {
        ModuleDefId::FunctionId(id)
    }
}
impl From<TypeDefId> for ModuleDefId {
    fn from(id: TypeDefId) -> Self {
        ModuleDefId::TypeDefId(id)
    }
}
impl From<RegisterId> for ModuleDefId {
    fn from(id: RegisterId) -> Self {
        ModuleDefId::RegisterId(id)
    }
}
impl From<ValSpecId> for ModuleDefId {
    fn from(id: ValSpecId) -> Self {
        ModuleDefId::ValSpecId(id)
    }
}
impl From<MappingId> for ModuleDefId {
    fn from(id: MappingId) -> Self {
        ModuleDefId::MappingId(id)
    }
}
impl From<LetId> for ModuleDefId {
    fn from(id: LetId) -> Self {
        ModuleDefId::LetId(id)
    }
}
impl From<OverloadId> for ModuleDefId {
    fn from(id: OverloadId) -> Self {
        ModuleDefId::OverloadId(id)
    }
}

impl From<FunctionId> for DefWithBodyId {
    fn from(id: FunctionId) -> Self {
        DefWithBodyId::FunctionId(id)
    }
}
impl From<MappingId> for DefWithBodyId {
    fn from(id: MappingId) -> Self {
        DefWithBodyId::MappingId(id)
    }
}
impl From<LetId> for DefWithBodyId {
    fn from(id: LetId) -> Self {
        DefWithBodyId::LetId(id)
    }
}

//
// These conversions enable gradual migration from raw DefId to typed IDs.
// Consumers can start using ModuleDefId in new code while old code
// continues with DefId.

impl ModuleDefId {
    /// Extract the raw `DefId` (for backward compat during migration).
    pub fn as_raw(&self) -> u32 {
        match self {
            ModuleDefId::FunctionId(id) => id.0,
            ModuleDefId::TypeDefId(id) => id.0,
            ModuleDefId::RegisterId(id) => id.0,
            ModuleDefId::ValSpecId(id) => id.0,
            ModuleDefId::MappingId(id) => id.0,
            ModuleDefId::LetId(id) => id.0,
            ModuleDefId::OverloadId(id) => id.0,
        }
    }

    /// Convert a raw `DefId` + `ItemKind` to a typed `ModuleDefId`.
    pub fn from_def_id(def_id: DefId, kind: crate::item_tree::ItemKind) -> Self {
        use crate::item_tree::ItemKind;
        let raw = def_id.0;
        match kind {
            ItemKind::Function => ModuleDefId::FunctionId(FunctionId(raw)),
            ItemKind::ValSpec => ModuleDefId::ValSpecId(ValSpecId(raw)),
            ItemKind::Mapping | ItemKind::MappingSpec => ModuleDefId::MappingId(MappingId(raw)),
            ItemKind::Register => ModuleDefId::RegisterId(RegisterId(raw)),
            ItemKind::Struct
            | ItemKind::Union
            | ItemKind::Enum
            | ItemKind::Bitfield
            | ItemKind::Newtype
            | ItemKind::TypeAlias => ModuleDefId::TypeDefId(TypeDefId(raw)),
            ItemKind::Let | ItemKind::Var => ModuleDefId::LetId(LetId(raw)),
            ItemKind::Overload => ModuleDefId::OverloadId(OverloadId(raw)),
            _ => ModuleDefId::FunctionId(FunctionId(raw)), // fallback
        }
    }
}
