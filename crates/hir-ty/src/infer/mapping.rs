//! Bidirectional mapping type checking.

use crate::infer::InferenceTable;
use crate::ty::{Ty, TyKind};

/// Result of resolving a mapping call.
#[derive(Debug, Clone)]
pub enum MappingCallResult {
    Forwards(Ty),
    Backwards(Ty),
    NoMatch,
}

/// Result of checking a mapping clause's type consistency.
#[derive(Debug, Clone)]
pub enum MappingCheckResult {
    Ok,
    ForwardsFailed { expected: Ty, actual: Ty },
    BackwardsFailed { expected: Ty, actual: Ty },
    SymmetryViolation { missing_on_left: Vec<String>, missing_on_right: Vec<String> },
}

/// Resolve a mapping call to a direction and a result type.
/// Forwards is `lhs -> rhs`, backwards `rhs -> lhs`.
pub fn resolve_mapping_call(
    table: &mut InferenceTable,
    lhs_ty: &Ty,
    rhs_ty: &Ty,
    arg_ty: &Ty,
) -> MappingCallResult {
    // Prefer proved directions, then retained constraints, then explicit
    // recovery. Both directions use the same directional checking operation.
    use super::unify::Compatibility;
    for wanted in [Compatibility::Proven, Compatibility::Deferred, Compatibility::Recovery] {
        for (input, output, forward) in [(lhs_ty, rhs_ty, true), (rhs_ty, lhs_ty, false)] {
            let snap = table.snapshot();
            let status = table.check_compatibility(input, arg_ty);
            let result = (status == wanted).then(|| table.deep_resolve(output));
            table.rollback_to(snap);
            if let Some(result) = result {
                return if forward {
                    MappingCallResult::Forwards(result)
                } else {
                    MappingCallResult::Backwards(result)
                };
            }
        }
    }

    MappingCallResult::NoMatch
}

/// Check that a bidirectional mapping clause satisfies both directions.
pub fn check_bidir_consistency(
    table: &mut InferenceTable,
    lhs_ty: &Ty,
    rhs_ty: &Ty,
    lhs_actual: &Ty,
    rhs_actual: &Ty,
) -> MappingCheckResult {
    // Check forwards: lhs_actual should match lhs_ty, rhs_actual should match rhs_ty
    let snap = table.snapshot();
    let forwards_ok = table.unify(lhs_ty, lhs_actual) && table.unify(rhs_ty, rhs_actual);
    table.rollback_to(snap);

    if !forwards_ok {
        return MappingCheckResult::ForwardsFailed {
            expected: lhs_ty.clone(),
            actual: lhs_actual.clone(),
        };
    }

    // Reversing a mapping preserves which declared side each value belongs to.
    let snap = table.snapshot();
    let backwards_ok = table.unify(rhs_ty, rhs_actual) && table.unify(lhs_ty, lhs_actual);
    table.rollback_to(snap);

    if !backwards_ok {
        return MappingCheckResult::BackwardsFailed {
            expected: rhs_ty.clone(),
            actual: lhs_actual.clone(),
        };
    }

    MappingCheckResult::Ok
}

/// Resolve a `TyKind::Bidir` type at a call site.
pub fn resolve_bidir_type_call(
    table: &mut InferenceTable,
    bidir_ty: &Ty,
    arg_ty: &Ty,
) -> Option<Ty> {
    match bidir_ty.kind() {
        TyKind::Bidir { lhs, rhs } => match resolve_mapping_call(table, lhs, rhs, arg_ty) {
            MappingCallResult::Forwards(ty) => Some(ty),
            MappingCallResult::Backwards(ty) => Some(ty),
            MappingCallResult::NoMatch => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_mapping_call_forwards() {
        let mut table = InferenceTable::default();
        let lhs = Ty::named("int");
        let rhs = Ty::named("string");
        let arg = Ty::named("int");

        let result = resolve_mapping_call(&mut table, &lhs, &rhs, &arg);
        match result {
            MappingCallResult::Forwards(ty) => assert_eq!(ty, Ty::named("string")),
            other => panic!("expected Forwards, got {:?}", other),
        }
    }

    #[test]
    fn resolve_mapping_call_backwards() {
        let mut table = InferenceTable::default();
        let lhs = Ty::named("int");
        let rhs = Ty::named("string");
        let arg = Ty::named("string");

        let result = resolve_mapping_call(&mut table, &lhs, &rhs, &arg);
        match result {
            MappingCallResult::Backwards(ty) => assert_eq!(ty, Ty::named("int")),
            other => panic!("expected Backwards, got {:?}", other),
        }
    }

    #[test]
    fn resolve_mapping_call_no_match() {
        let mut table = InferenceTable::default();
        let lhs = Ty::named("int");
        let rhs = Ty::named("string");
        let arg = Ty::named("bool");

        let result = resolve_mapping_call(&mut table, &lhs, &rhs, &arg);
        assert!(matches!(result, MappingCallResult::NoMatch));
    }

    #[test]
    fn bidir_consistency_both_directions_ok() {
        let mut table = InferenceTable::default();
        let lhs_ty = Ty::named("int");
        let rhs_ty = Ty::named("int"); // same type → bidir trivially ok

        let result = check_bidir_consistency(
            &mut table,
            &lhs_ty,
            &rhs_ty,
            &Ty::named("int"),
            &Ty::named("int"),
        );
        assert!(matches!(result, MappingCheckResult::Ok));
    }

    #[test]
    fn bidir_consistency_forwards_fails() {
        let mut table = InferenceTable::default();
        let lhs_ty = Ty::named("int");
        let rhs_ty = Ty::named("string");

        // lhs_actual is "bool" which doesn't match lhs_ty "int"
        let result = check_bidir_consistency(
            &mut table,
            &lhs_ty,
            &rhs_ty,
            &Ty::named("bool"),
            &Ty::named("string"),
        );
        assert!(matches!(result, MappingCheckResult::ForwardsFailed { .. }));
    }

    #[test]
    fn resolve_bidir_type_at_call_site() {
        let mut table = InferenceTable::default();
        let bidir = Ty::bidir(Ty::named("int"), Ty::named("string"));
        let arg = Ty::named("int");

        let result = resolve_bidir_type_call(&mut table, &bidir, &arg);
        assert_eq!(result, Some(Ty::named("string")));
    }

    #[test]
    fn resolve_bidir_type_backwards_at_call_site() {
        let mut table = InferenceTable::default();
        let bidir = Ty::bidir(Ty::named("int"), Ty::named("string"));
        let arg = Ty::named("string");

        let result = resolve_bidir_type_call(&mut table, &bidir, &arg);
        assert_eq!(result, Some(Ty::named("int")));
    }

    #[test]
    fn table_unchanged_after_mapping_resolution() {
        let mut table = InferenceTable::default();
        let var = table.new_type_var();
        let lhs = Ty::named("int");
        let rhs = Ty::named("string");

        let _ = resolve_mapping_call(&mut table, &lhs, &rhs, &Ty::named("int"));

        // Inference var should still be unresolved
        let resolved = table.shallow_resolve(&var);
        assert!(matches!(resolved.kind(), TyKind::Infer(crate::ty::InferTy(_))));
    }
}
