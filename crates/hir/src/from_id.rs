//! Conversion between internal IDs and public HIR types.
//! Provides `From` impls for converting between `DefLocation`
//! and the public wrapper types (`Function`, `Adt`, etc.).

use hir_def::item_tree::ItemKind;

use crate::{Adt, DefLocation, Function, Mapping, ModuleDef, Register, TypeDefKind};

/// Generate `From<DefLocation> for $ty` and `From<$ty> for DefLocation`
/// for each wrapper type that has an `id: DefLocation` field.
macro_rules! from_id {
    ($(($ty:ty)),* $(,)?) => {$(
        impl From<DefLocation> for $ty {
            fn from(id: DefLocation) -> Self {
                Self { id }
            }
        }
        impl From<$ty> for DefLocation {
            fn from(it: $ty) -> Self {
                it.id
            }
        }
    )*}
}

from_id![(Function), (Register), (Mapping),];

// Adt has an extra `kind` field, so it only converts one way
// (Adt → DefLocation). DefLocation → Adt needs a kind.
impl From<Adt> for DefLocation {
    fn from(t: Adt) -> Self {
        t.id
    }
}

macro_rules! from_module_def {
    ($(($ty:ty, $variant:ident)),* $(,)?) => {$(
        impl From<$ty> for ModuleDef {
            fn from(it: $ty) -> Self {
                ModuleDef::$variant(it)
            }
        }
    )*}
}

from_module_def![
    (Function, Function),
    (Adt, TypeDef),
    (Register, Register),
    (Mapping, Mapping),
];

impl ModuleDef {
    /// Construct a `ModuleDef` from a `DefLocation` and `ItemKind`.
    ///
    /// This is the Sail equivalent of RA's `from_id::from_module_def_id`.
    pub fn from_def_location(loc: DefLocation, kind: ItemKind) -> Self {
        match kind {
            ItemKind::Function => ModuleDef::Function(Function { id: loc }),
            ItemKind::Mapping | ItemKind::MappingSpec => ModuleDef::Mapping(Mapping { id: loc }),
            ItemKind::Register => ModuleDef::Register(Register { id: loc }),
            ItemKind::Struct
            | ItemKind::Union
            | ItemKind::Enum
            | ItemKind::Bitfield
            | ItemKind::Newtype
            | ItemKind::TypeAlias => {
                let kind = TypeDefKind::from_item_kind(kind).unwrap_or(TypeDefKind::TypeAlias);
                ModuleDef::TypeDef(Adt { id: loc, kind })
            }
            ItemKind::Let | ItemKind::Var => ModuleDef::Let(loc),
            ItemKind::Overload => ModuleDef::Overload(loc),
            ItemKind::ValSpec => ModuleDef::Function(Function { id: loc }),
            _ => ModuleDef::Let(loc), // ScatteredHead, ScatteredClause, etc.
        }
    }
}

// ModuleDef::location() is defined in lib.rs alongside the enum definition.
