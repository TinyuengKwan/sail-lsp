//! Conversion between internal IDs and public HIR types.
//! Provides `From` impls for converting between `DefLocation`
//! and the public wrapper types (`Function`, `TypeDef`, etc.).

use crate::{Def, DefKind, DefLocation, Function, Mapping, Register, TypeDef, TypeDefKind};

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

// TypeDef has an extra `kind` field, so it only converts one way
// (TypeDef → DefLocation). DefLocation → TypeDef needs a kind.
impl From<TypeDef> for DefLocation {
    fn from(t: TypeDef) -> Self {
        t.id
    }
}

macro_rules! from_def {
    ($(($ty:ty, $variant:ident)),* $(,)?) => {$(
        impl From<$ty> for Def {
            fn from(it: $ty) -> Self {
                Def::$variant(it)
            }
        }
    )*}
}

from_def![(Function, Function), (TypeDef, TypeDef), (Register, Register), (Mapping, Mapping),];

impl Def {
    /// Classify a definition at `loc`.
    ///
    /// Total, and with no arm that stands in for another: every `DefKind`
    /// has a `Def` variant of its own. It did not use to — a `val` came
    /// back as a `Function` and the eight remaining kinds all came back as
    /// `Let` — which is why callers reached past `Def` for the raw kind.
    pub fn from_def_location(loc: DefLocation, kind: DefKind) -> Self {
        match kind {
            DefKind::Function => Def::Function(Function { id: loc }),
            DefKind::ValSpec => Def::ValSpec(loc),
            DefKind::Mapping | DefKind::MappingSpec => Def::Mapping(Mapping { id: loc }),
            DefKind::Register => Def::Register(Register { id: loc }),
            DefKind::Struct
            | DefKind::Union
            | DefKind::Enum
            | DefKind::Bitfield
            | DefKind::Newtype
            | DefKind::TypeAlias => {
                let kind = TypeDefKind::from_item_kind(kind).unwrap_or(TypeDefKind::TypeAlias);
                Def::TypeDef(TypeDef { id: loc, kind })
            }
            DefKind::Let | DefKind::Var => Def::Let(loc),
            DefKind::Overload => Def::Overload(loc),
            DefKind::ScatteredHead => Def::ScatteredHead(loc),
            DefKind::ScatteredClause => Def::ScatteredClause(loc),
            DefKind::EndMarker => Def::EndMarker(loc),
            DefKind::Constraint => Def::Constraint(loc),
            DefKind::TerminationMeasure => Def::TerminationMeasure(loc),
            DefKind::Instantiation => Def::Instantiation(loc),
            DefKind::Outcome => Def::Outcome(loc),
            DefKind::Field => Def::Field(loc),
            DefKind::Variant => Def::Variant(loc),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::{FileId, FileText};
    use hir_def::DefId;
    use std::sync::Arc;

    /// Every kind Sail can classify has a `Def` of its own.
    ///
    /// The two collapses below are the language's, not ours: `var` is a
    /// mutable `let`, and a bare `mapping` signature is a `mapping`. Any
    /// other pair that round-trips to a different kind means one form is
    /// standing in for another — a `val` coming back as a `Function`, the
    /// rest coming back as `Let` — which is what this test exists to catch.
    #[test]
    fn every_def_kind_round_trips() {
        let db = crate::hir_query::tests::TestDb::default();
        let file_text = FileText::new(&db, Arc::from(""), FileId::from_raw(0));
        let loc = DefLocation { file_text, def_id: DefId::LetId(hir_def::LetId(0)) };

        for kind in DEF_KINDS {
            let expected = match kind {
                DefKind::Var => DefKind::Let,
                DefKind::MappingSpec => DefKind::Mapping,
                other => other,
            };
            let got = Def::from_def_location(loc, kind).kind();
            assert_eq!(got, expected, "{kind:?} came back as {got:?}");
        }
    }

    const DEF_KINDS: [DefKind; 23] = [
        DefKind::Function,
        DefKind::Mapping,
        DefKind::ValSpec,
        DefKind::MappingSpec,
        DefKind::TypeAlias,
        DefKind::Struct,
        DefKind::Union,
        DefKind::Enum,
        DefKind::Bitfield,
        DefKind::Newtype,
        DefKind::Register,
        DefKind::Let,
        DefKind::Var,
        DefKind::Overload,
        DefKind::ScatteredHead,
        DefKind::ScatteredClause,
        DefKind::Constraint,
        DefKind::TerminationMeasure,
        DefKind::EndMarker,
        DefKind::Instantiation,
        DefKind::Outcome,
        DefKind::Field,
        DefKind::Variant,
    ];
}
