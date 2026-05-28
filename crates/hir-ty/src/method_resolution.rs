//! Method/overload resolution for Sail function calls.
//!
//! dispatch with probe/confirm phases; Sail uses ad-hoc overloading where
//! multiple functions share the same name and are selected by argument types.
//!
//! ## Resolution algorithm (with InferenceTable snapshots)
//!
//! 1. **Arity filter**: reject candidates with wrong parameter count.
//! 2. **Type unification with snapshots**: for each candidate, take an
//!    InferenceTable snapshot, try to unify all argument types with
//!    parameter types. If unification succeeds, the candidate is viable.
//!    On failure, rollback.
//! 3. **Return-type filter**: if a return type expectation is provided,
//!    filter candidates whose return type doesn't unify with the expected.
//! 4. **Specificity**: if multiple candidates remain, pick the most
//!    specific (fewest type variables after unification).
//!
//! ## RA counterpart
//!
//! RA's `method_resolution.rs`:
//! - Takes `&mut InferenceTable` for speculative unification
//! - Uses `table.snapshot()` / `table.rollback_to()` pattern
//! - Filters by self-type unification, then by trait bounds

use base_db::FileId;
use hir_def::name::Name;

use crate::infer::InferenceTable;
use crate::ty::{Ty, TyKind};

/// In RA this holds `InferCtxt`, `Resolver`, `ParamEnv`, `traits_in_scope`,
/// `edition`, `unstable_features`. In Sail there are no traits/impls, so the
/// context is minimal: just the inference table for speculative unification.
///
/// carries only what's needed for type-directed candidate pruning.
#[derive(Debug)]
pub struct MethodResolutionContext<'a> {
    /// Inference table for speculative unification (snapshot/rollback).
    pub table: &'a mut InferenceTable,
}

impl<'a> MethodResolutionContext<'a> {
    /// Try unifying argument types with a candidate's parameter types.
    /// Returns true if unification succeeds (candidate is viable).
    /// Uses snapshot/rollback on the inference table: on failure the table
    /// state is restored; on success the bindings are kept so the caller
    /// can inspect the unified state (and rollback its own outer snapshot
    /// if needed).
    pub fn try_candidate(&mut self, params: &[Ty], args: &[Ty]) -> bool {
        let snap = self.table.snapshot();
        let mut ok = true;
        for (expected, actual) in params.iter().zip(args.iter()) {
            if !self.table.unify(expected, actual) {
                ok = false;
                break;
            }
        }
        if !ok {
            self.table.rollback_to(snap);
        }
        ok
    }
}

/// A single candidate for overload resolution.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Function name.
    pub name: Name,
    /// File where this candidate is defined.
    pub file_id: FileId,
    /// Parameter types of this candidate.
    pub params: Vec<Ty>,
    /// Return type of this candidate.
    pub ret: Ty,
    /// Declared effects (for effect checking).
    pub declared_effects: Vec<String>,
}

/// Set of candidates for a given overloaded name.
#[derive(Debug, Clone, Default)]
pub struct Candidates {
    candidates: Vec<Candidate>,
}

impl Candidates {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, candidate: Candidate) {
        self.candidates.push(candidate);
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter()
    }
}

/// Result of overload resolution.
#[derive(Debug)]
pub enum Pick<'a> {
    /// Exactly one candidate matches.
    Resolved(&'a Candidate),
    /// Multiple candidates match (ambiguous).
    Ambiguous(Vec<&'a Candidate>),
    /// No candidate matches.
    NoMatch,
}

/// Resolve an overloaded function call using InferenceTable snapshots.
///
/// ## Arguments
///
/// - `name`: the function name
/// - `arg_count`: number of arguments at the call site
/// - `arg_tys`: argument types (may contain inference variables)
/// - `expected_ret`: optional return type expectation (for filtering)
/// - `candidates`: all candidates for this name
/// - `table`: the inference table for speculative unification
///
/// ## Algorithm
///
/// 1. Arity filter
/// 2. For each surviving candidate: snapshot → try unify all args → commit/rollback
/// 3. Return-type filter (if expected_ret provided)
/// 4. Specificity ranking
pub fn resolve_overload_with_table<'a>(
    _name: &str,
    arg_count: usize,
    arg_tys: &[Ty],
    expected_ret: Option<&Ty>,
    candidates: &'a Candidates,
    table: &mut InferenceTable,
) -> Pick<'a> {
    if candidates.is_empty() {
        return Pick::NoMatch;
    }

    // Stage 1: Arity filter
    let arity_matches: Vec<_> = candidates.iter().filter(|c| c.params.len() == arg_count).collect();

    if arity_matches.is_empty() {
        return Pick::NoMatch;
    }

    if arity_matches.len() == 1 {
        return Pick::Resolved(arity_matches[0]);
    }

    // Stage 2: Type unification with snapshot/rollback
    //
    // InferenceTable, try to unify all argument types with parameter
    // types. If it succeeds, the candidate is viable. If it fails,
    // rollback and try the next candidate.
    let mut viable: Vec<(&Candidate, usize)> = Vec::new(); // (candidate, specificity_score)

    for &candidate in &arity_matches {
        let snap = table.snapshot();
        let mut all_unified = true;

        for (param, arg) in candidate.params.iter().zip(arg_tys.iter()) {
            if !table.unify(param, arg) {
                all_unified = false;
                break;
            }
        }

        if all_unified {
            // Compute specificity: count how many params are concrete (not variables)
            let specificity = candidate.params.iter().filter(|p| !is_var_like(p)).count();
            viable.push((candidate, specificity));
            // Rollback — we don't commit yet (need to compare all candidates)
            table.rollback_to(snap);
        } else {
            table.rollback_to(snap);
        }
    }

    if viable.is_empty() {
        return Pick::NoMatch;
    }

    // Stage 3: Return-type filter
    //
    // the surrounding context), filter candidates whose return type
    // doesn't unify with it.
    if let Some(expected) = expected_ret {
        if !expected.is_error() {
            let before_len = viable.len();
            viable.retain(|(candidate, _)| {
                let snap = table.snapshot();
                let ok = table.unify(&candidate.ret, expected);
                table.rollback_to(snap);
                ok
            });
            // If filtering removed all candidates, restore them
            // (better to be ambiguous than to report no-match)
            if viable.is_empty() && before_len > 0 {
                // Re-run without return type filter
                for &candidate in &arity_matches {
                    let snap = table.snapshot();
                    let mut all_unified = true;
                    for (param, arg) in candidate.params.iter().zip(arg_tys.iter()) {
                        if !table.unify(param, arg) {
                            all_unified = false;
                            break;
                        }
                    }
                    if all_unified {
                        let specificity =
                            candidate.params.iter().filter(|p| !is_var_like(p)).count();
                        viable.push((candidate, specificity));
                    }
                    table.rollback_to(snap);
                }
            }
        }
    }

    if viable.is_empty() {
        return Pick::NoMatch;
    }

    // Stage 4: Specificity ranking
    //
    // Sort by specificity (most specific first). If the top candidate
    // is strictly more specific than the second, resolve to it.
    viable.sort_by_key(|b| std::cmp::Reverse(b.1));

    if viable.len() == 1 {
        return Pick::Resolved(viable[0].0);
    }

    if viable[0].1 > viable[1].1 {
        // Clear winner by specificity
        Pick::Resolved(viable[0].0)
    } else {
        // Multiple candidates with same specificity — ambiguous
        Pick::Ambiguous(viable.iter().map(|(c, _)| *c).collect())
    }
}

/// Legacy API: resolve without InferenceTable (backward-compat).
///
/// Uses simplified type compatibility checks instead of proper
/// unification. New code should prefer `resolve_overload_with_table`.
pub fn resolve_overload<'a>(
    name: &str,
    arg_count: usize,
    arg_tys: &[Ty],
    candidates: &'a Candidates,
) -> Pick<'a> {
    let mut table = InferenceTable::default();
    resolve_overload_with_table(name, arg_count, arg_tys, None, candidates, &mut table)
}

/// Check if a type is a type variable or inference variable.
fn is_var_like(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Param(_) | TyKind::Infer(crate::ty::InferTy(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, params: &[&str], ret: &str) -> Candidate {
        Candidate {
            name: Name::new(name),
            file_id: FileId::from_raw(0),
            params: params.iter().map(|s| Ty::named(*s)).collect(),
            ret: Ty::named(ret),
            declared_effects: Vec::new(),
        }
    }

    #[test]
    fn single_candidate_resolves() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("f", &["int"], "bool"));
        let result = resolve_overload("f", 1, &[Ty::named("int")], &candidates);
        assert!(matches!(result, Pick::Resolved(_)));
    }

    #[test]
    fn arity_mismatch_no_match() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("f", &["int", "int"], "bool"));
        let result = resolve_overload("f", 1, &[Ty::named("int")], &candidates);
        assert!(matches!(result, Pick::NoMatch));
    }

    #[test]
    fn type_filter_narrows() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("f", &["int"], "int"));
        candidates.push(candidate("f", &["bool"], "bool"));
        let result = resolve_overload("f", 1, &[Ty::named("int")], &candidates);
        assert!(matches!(result, Pick::Resolved(c) if c.ret == Ty::named("int")));
    }

    #[test]
    fn empty_candidates_no_match() {
        let candidates = Candidates::new();
        let result = resolve_overload("f", 0, &[], &candidates);
        assert!(matches!(result, Pick::NoMatch));
    }

    #[test]
    fn snapshot_unification_resolves_correctly() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("add", &["int", "int"], "int"));
        candidates.push(candidate("add", &["bool", "bool"], "bool"));

        let mut table = InferenceTable::default();
        let result = resolve_overload_with_table(
            "add",
            2,
            &[Ty::named("int"), Ty::named("int")],
            None,
            &candidates,
            &mut table,
        );
        match result {
            Pick::Resolved(c) => {
                assert_eq!(c.ret, Ty::named("int"));
            }
            other => panic!("expected Resolved, got {:?}", other),
        }
    }

    #[test]
    fn return_type_filter_selects_correct_candidate() {
        let mut candidates = Candidates::new();
        // Both accept "int" arg, but return different types
        candidates.push(candidate("convert", &["int"], "string"));
        candidates.push(candidate("convert", &["int"], "bool"));

        let mut table = InferenceTable::default();
        let result = resolve_overload_with_table(
            "convert",
            1,
            &[Ty::named("int")],
            Some(&Ty::named("bool")), // expect bool return
            &candidates,
            &mut table,
        );
        match result {
            Pick::Resolved(c) => {
                assert_eq!(c.ret, Ty::named("bool"));
            }
            other => panic!("expected Resolved with bool return, got {:?}", other),
        }
    }

    #[test]
    fn specificity_prefers_concrete_params() {
        let mut candidates = Candidates::new();
        // Candidate 1: concrete param "int"
        candidates.push(candidate("f", &["int"], "int"));
        // Candidate 2: generic param (type variable)
        candidates.push(Candidate {
            name: Name::new("f"),
            file_id: FileId::from_raw(0),
            params: vec![Ty::param("'a")],
            ret: Ty::param("'a"),
            declared_effects: Vec::new(),
        });

        let mut table = InferenceTable::default();
        let result =
            resolve_overload_with_table("f", 1, &[Ty::named("int")], None, &candidates, &mut table);
        match result {
            Pick::Resolved(c) => {
                // Should prefer the concrete "int" candidate over generic "'a"
                assert_eq!(c.ret, Ty::named("int"));
            }
            other => panic!("expected Resolved(int), got {:?}", other),
        }
    }

    #[test]
    fn table_state_unchanged_after_resolution() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("f", &["int"], "int"));
        candidates.push(candidate("f", &["bool"], "bool"));

        let mut table = InferenceTable::default();
        let var = table.new_type_var();

        // Resolve — should not affect the inference variable
        let _result =
            resolve_overload_with_table("f", 1, &[Ty::named("int")], None, &candidates, &mut table);

        // The inference variable should still be unresolved
        let resolved = table.shallow_resolve(&var);
        assert!(matches!(resolved.kind(), TyKind::Infer(crate::ty::InferTy(_))));
    }

    #[test]
    fn ambiguous_when_equal_specificity() {
        let mut candidates = Candidates::new();
        candidates.push(candidate("f", &["int"], "A"));
        candidates.push(candidate("f", &["int"], "B"));

        let mut table = InferenceTable::default();
        let result =
            resolve_overload_with_table("f", 1, &[Ty::named("int")], None, &candidates, &mut table);
        assert!(matches!(result, Pick::Ambiguous(_)));
    }
}
