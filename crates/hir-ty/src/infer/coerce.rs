//! # Type Coercion
//! Under certain circumstances we will coerce from one type to another.
//! In Sail this occurs when:
//!
//! - A numeric subtype is used where its supertype is expected
//!   (e.g., `nat` where `int` is expected, `atom(5)` where `range(0, 10)` is expected)
//! - Bitvector widths are proved equal by constraints
//! - An existential type is unpacked
//!
//! This module provides the entry point `try_coerce` that is called
//! by the inference engine when `unify()` fails, delegating to
//! `subtype::is_subtype` for the actual compatibility check.

use super::subtype::{is_subtype, SubtypeResult};
use super::{InferenceTable, Ty, TyKind};

/// The kind of coercion that was applied.
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoercionKind {
    /// Numeric subtype coercion: `nat → int`, `atom(N) → int`,
    /// `range(lo, hi) → int`, `atom_bool → bool`.
    NumericSubtype,
    /// Range subsumption: `range(lo1, hi1) → range(lo2, hi2)`
    /// where `lo2 <= lo1` and `hi1 <= hi2`.
    RangeSubsume,
    /// Bitvector width equality: `bits(N) → bits(M)` where N == M.
    /// (This is identity — not actually widening, but validates the constraint.)
    BitvectorWidthEqual,
    /// Tuple coercion: component-wise coercion.
    TupleCoerce,
    /// Abstract type coercion: `abstract(Kind::Int) → int`, etc.
    AbstractKind,
}

/// Result of attempting a coercion.
#[derive(Clone, Debug)]
pub enum CoerceResult {
    /// Coercion succeeded — the `from` type can be implicitly converted to `to`.
    ///
    /// The kind is read by this crate's tests and by nothing else yet;
    /// `expect` cannot say that, because the lint fires in the lib build
    /// and not in the test build.
    Ok(#[allow(dead_code)] CoercionKind),
    /// Accepted for inference recovery; a retained obligation is still unproved.
    Deferred(#[allow(dead_code)] CoercionKind),
    /// Coercion failed — the types are incompatible.
    Fail,
}

impl CoerceResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, CoerceResult::Ok(_) | CoerceResult::Deferred(_))
    }
}

/// Attempt to coerce `from_ty` to `to_ty`.
/// This is called by the inference engine when `unify()` alone fails.
/// It uses `subtype::is_subtype` to check if `from_ty` is a subtype
/// of `to_ty` under Sail's numeric subtyping rules.
///
/// Returns `Ok` for a proof, `Deferred` for an outstanding obligation,
/// or `Fail` for an incompatible relation.
pub(super) fn try_coerce(table: &mut InferenceTable, from_ty: &Ty, to_ty: &Ty) -> CoerceResult {
    let from = table.shallow_resolve(from_ty);
    let to = table.shallow_resolve(to_ty);

    // Fast path: if types are already equal, no coercion needed.
    if from == to {
        return CoerceResult::Ok(CoercionKind::NumericSubtype);
    }

    // Conversion is directional: actual <: expected. A reverse conversion
    // cannot rescue a failed narrowing check.
    match is_subtype(table, &from, &to) {
        SubtypeResult::Ok => CoerceResult::Ok(classify_coercion(&from, &to)),
        SubtypeResult::NeedsConstraint(obligation) => {
            table.push_subtype_obligation(obligation);
            CoerceResult::Deferred(classify_coercion(&from, &to))
        }
        SubtypeResult::Fail => CoerceResult::Fail,
    }
}

/// Classify what kind of coercion is happening between two types.
fn classify_coercion(from: &Ty, to: &Ty) -> CoercionKind {
    let from_name = from.as_name();
    let to_name = to.as_name();
    match (from.kind(), to.kind()) {
        // nat/atom/range → int
        _ if from_name == Some("nat") && to_name == Some("int") => CoercionKind::NumericSubtype,
        (TyKind::App { name, .. }, _)
            if (name == "atom" || name == "range" || name == "nat") && to_name == Some("int") =>
        {
            CoercionKind::NumericSubtype
        }
        // atom_bool → bool
        (TyKind::App { name, .. }, _) if name == "atom_bool" && to_name == Some("bool") => {
            CoercionKind::NumericSubtype
        }
        // range → range (subsumption)
        (TyKind::App { name: n1, .. }, TyKind::App { name: n2, .. })
            if n1 == "range" && n2 == "range" =>
        {
            CoercionKind::RangeSubsume
        }
        // bits → bits (width check)
        (TyKind::App { name: n1, .. }, TyKind::App { name: n2, .. })
            if n1 == "bits" && n2 == "bits" =>
        {
            CoercionKind::BitvectorWidthEqual
        }
        // tuple → tuple
        (TyKind::Tuple(_), TyKind::Tuple(_)) => CoercionKind::TupleCoerce,
        // abstract → scalar/adt
        (TyKind::Abstract { .. }, TyKind::Scalar(_) | TyKind::Named(_, _)) => {
            CoercionKind::AbstractKind
        }
        // Default
        _ => CoercionKind::NumericSubtype,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_coerces_to_int() {
        let mut table = InferenceTable::default();
        let nat = Ty::named("nat");
        let int = Ty::named("int");
        let result = try_coerce(&mut table, &nat, &int);
        assert!(result.is_ok());
        match result {
            CoerceResult::Ok(kind) => assert_eq!(kind, CoercionKind::NumericSubtype),
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn unrefined_int_does_not_coerce_to_nat() {
        let mut table = InferenceTable::default();
        assert!(!try_coerce(&mut table, &Ty::named("int"), &Ty::named("nat")).is_ok());
    }

    #[test]
    fn same_type_coerces_trivially() {
        let mut table = InferenceTable::default();
        let int = Ty::named("int");
        let result = try_coerce(&mut table, &int, &int);
        assert!(result.is_ok());
    }

    #[test]
    fn unrelated_types_fail() {
        let mut table = InferenceTable::default();
        let int = Ty::named("int");
        let bool_ty = Ty::named("bool");
        let result = try_coerce(&mut table, &int, &bool_ty);
        assert!(!result.is_ok());
    }
}
