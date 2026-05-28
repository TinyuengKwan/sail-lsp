//! Subtype relation for Sail's type system.
//!
//! Sail's subtyping is structural (range <: int, bits(N) <: bits(M)
//! when N == M), with compatibility checks that may generate
//! constraint obligations.

use super::nexp_simplify::{check_eq, check_le, try_eval_const};
use super::*;

/// Try Z3 to verify a numeric comparison when algebraic simplification fails.
/// Returns `Some(true)` if provably true, `None` otherwise (Z3 not available,
/// timeout, or undecidable). Never returns `Some(false)` — LSP is permissive.
fn z3_check_le(a: &NumericExpr, b: &NumericExpr) -> Option<bool> {
    // Construct constraint: a <= b
    let constraint = ConstraintExpr::Compare { lhs: a.clone(), op: CompareOp::Lte, rhs: b.clone() };
    let subst = Subst::default();
    match super::z3_solver::try_solve(&constraint, &subst, &[]) {
        ConstraintStatus::Satisfied => Some(true),
        _ => None, // Unknown or Violated — don't return false (permissive)
    }
}

/// Try Z3 to verify numeric equality.
fn z3_check_eq(a: &NumericExpr, b: &NumericExpr) -> Option<bool> {
    let constraint = ConstraintExpr::Compare { lhs: a.clone(), op: CompareOp::Eq, rhs: b.clone() };
    let subst = Subst::default();
    match super::z3_solver::try_solve(&constraint, &subst, &[]) {
        ConstraintStatus::Satisfied => Some(true),
        _ => None,
    }
}

/// Extract a `NumericExpr` from a `TyArg`, trying the structured form first
/// and falling back to parsing the string representation.
fn extract_nexp(arg: &TyArg) -> Option<NumericExpr> {
    match arg {
        TyArg::Nexp(n) => Some(n.clone()),
        TyArg::Value(s) => NumericExpr::parse(s),
        TyArg::Type(_) => None,
    }
}

/// Result of a subtype check.
///
/// `NoSolution` → Fail, needs-solving → NeedsConstraint.
#[derive(Clone, Debug)]
pub(super) enum SubtypeResult {
    /// sub <: sup holds unconditionally.
    Ok,
    /// sub <: sup holds if the given constraint is satisfied.
    NeedsConstraint(Obligation),
    /// sub <: sup does not hold.
    Fail,
}

/// Check whether `sub` is a subtype of `sup`.
///
/// Implements Sail's structural subtyping rules:
/// - `range(lo, hi) <: int` — always holds
/// - `atom('n) <: int` — always holds
/// - `nat <: int` — always holds
/// - `bits(N) <: bits(M)` — requires N == M (width equality)
/// - Otherwise: falls back to symmetric unification.
pub(super) fn is_subtype(table: &mut InferenceTable, sub: &Ty, sup: &Ty) -> SubtypeResult {
    let sub = table.shallow_resolve(sub);
    let sup = table.shallow_resolve(sup);
    let sub = table.normalize_alias_ty(&sub);
    let sup = table.normalize_alias_ty(&sup);

    // Use as_name() for comparisons that work across Scalar/Adt
    let sub_name = sub.as_name();
    let sup_name = sup.as_name();

    // Single-element tuple unwrap: (T) ≡ T in Sail's type system.
    // CST lowering can produce these from parenthesized expressions.
    match (sub.kind(), sup.kind()) {
        (TyKind::Tuple(items), _) if items.len() == 1 => {
            return is_subtype(table, &items[0], &sup);
        }
        (_, TyKind::Tuple(items)) if items.len() == 1 => {
            return is_subtype(table, &sub, &items[0]);
        }
        _ => {}
    }

    // bool(constraint) ≡ bool — App("bool", [X]) is bool with a constraint
    // parameter that arises from cross-file return type propagation.
    match (sub.kind(), sup.kind()) {
        (TyKind::App { name, .. }, _) if name == "bool" && sup_name == Some("bool") => {
            return SubtypeResult::Ok;
        }
        (_, TyKind::App { name, .. }) if name == "bool" && sub_name == Some("bool") => {
            return SubtypeResult::Ok;
        }
        _ => {}
    }

    match (sub.kind(), sup.kind()) {
        (TyKind::Scalar(a), TyKind::Scalar(b)) if a == b => SubtypeResult::Ok,
        (TyKind::Adt(a, _), TyKind::Adt(b, _)) if a == b => SubtypeResult::Ok,

        //
        // range(lo, hi) <: int — always holds
        // atom('n) <: int — always holds
        // nat <: int — always holds
        (TyKind::App { name: app_name, .. }, _)
            if (app_name == "range"
                || app_name == "atom"
                || app_name == "nat"
                || app_name == "implicit")
                && sup_name == Some("int") =>
        {
            SubtypeResult::Ok
        }

        // implicit('n) <-> int: implicit params are integers inferred at
        // compile time; explicit int values satisfy implicit parameters.
        _ if sup_name == Some("int")
            && matches!(sub.kind(), TyKind::App { name, .. } if name == "implicit") =>
        {
            SubtypeResult::Ok
        }
        (_, TyKind::App { name: app_name, .. })
            if app_name == "implicit" && sub_name == Some("int") =>
        {
            SubtypeResult::Ok
        }
        // implicit(N) <-> int(M) — App variants with numeric args.
        (TyKind::App { name: sub_app, .. }, TyKind::App { name: sup_app, .. })
            if (sub_app == "int" && sup_app == "implicit")
                || (sub_app == "implicit" && sup_app == "int") =>
        {
            SubtypeResult::Ok
        }

        // nat <: int — always holds
        _ if sub_name == Some("nat") && sup_name == Some("int") => SubtypeResult::Ok,

        // int <: nat — accept permissively since the compiler verifies
        // constraints. Concrete negative values are still rejected.
        _ if sub_name == Some("int") && sup_name == Some("nat") => {
            if let TyKind::App { name, args, .. } = sub.kind() {
                if name == "atom" {
                    if let Some(nexp) = args.first().and_then(extract_nexp) {
                        if let Some(val) = try_eval_const(&nexp) {
                            if val < 0 {
                                return SubtypeResult::Fail;
                            }
                        }
                    }
                }
            }
            SubtypeResult::Ok
        }

        // atom_bool <: bool
        (TyKind::App { name: app_name, .. }, _)
            if app_name == "atom_bool" && sup_name == Some("bool") =>
        {
            SubtypeResult::Ok
        }

        // atom(constraint_expr) <: bool — sizeof comparisons produce atom(...)
        // that should coerce to bool.
        (TyKind::App { name: app_name, .. }, _)
            if app_name == "atom" && sup_name == Some("bool") =>
        {
            SubtypeResult::Ok
        }

        // atom(N) <: range(lo, hi) — holds when lo <= N <= hi
        (
            TyKind::App { name: sub_name, args: sub_args, .. },
            TyKind::App { name: sup_name, args: sup_args, .. },
        ) if sub_name == "atom" && sup_name == "range" => {
            let sub_nexp = sub_args.first().and_then(extract_nexp);
            let sup_lo_nexp = sup_args.first().and_then(extract_nexp);
            let sup_hi_nexp = sup_args.get(1).and_then(extract_nexp);

            match (sub_nexp, sup_lo_nexp, sup_hi_nexp) {
                (Some(n_expr), Some(lo_expr), Some(hi_expr)) => {
                    // Try constant folding first
                    let n = try_eval_const(&n_expr);
                    let lo = try_eval_const(&lo_expr);
                    let hi = try_eval_const(&hi_expr);
                    match (n, lo, hi) {
                        (Some(n), Some(lo), Some(hi)) if lo <= n && n <= hi => SubtypeResult::Ok,
                        (Some(_), Some(_), Some(_)) => SubtypeResult::Fail,
                        _ => {
                            // Try algebraic reasoning: lo <= n and n <= hi
                            let lo_le_n = check_le(&lo_expr, &n_expr)
                                .or_else(|| z3_check_le(&lo_expr, &n_expr));
                            let n_le_hi = check_le(&n_expr, &hi_expr)
                                .or_else(|| z3_check_le(&n_expr, &hi_expr));
                            match (lo_le_n, n_le_hi) {
                                (Some(true), Some(true)) => SubtypeResult::Ok,
                                (Some(false), _) | (_, Some(false)) => SubtypeResult::Fail,
                                _ => {
                                    // Undecidable — permissive for LSP.
                                    if table.raw_unify(&sub, &sup) {
                                        SubtypeResult::Ok
                                    } else {
                                        SubtypeResult::Fail
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    if table.raw_unify(&sub, &sup) {
                        SubtypeResult::Ok
                    } else {
                        SubtypeResult::Fail
                    }
                }
            }
        }

        // atom(N) <: atom(M) — holds when N == M
        (TyKind::App { name: n1, args: a1, .. }, TyKind::App { name: n2, args: a2, .. })
            if n1 == "atom" && n2 == "atom" =>
        {
            let v1 = a1.first().and_then(extract_nexp);
            let v2 = a2.first().and_then(extract_nexp);
            match (v1, v2) {
                (Some(e1), Some(e2)) => match check_eq(&e1, &e2) {
                    Some(true) => SubtypeResult::Ok,
                    Some(false) => SubtypeResult::Fail,
                    None => {
                        if table.raw_unify(&sub, &sup) {
                            SubtypeResult::Ok
                        } else {
                            SubtypeResult::Fail
                        }
                    }
                },
                _ => {
                    if table.raw_unify(&sub, &sup) {
                        SubtypeResult::Ok
                    } else {
                        SubtypeResult::Fail
                    }
                }
            }
        }

        // range(lo1, hi1) <: range(lo2, hi2) — requires lo2 <= lo1 && hi1 <= hi2
        (
            TyKind::App { name: sub_name, args: sub_args, .. },
            TyKind::App { name: sup_name, args: sup_args, .. },
        ) if sub_name == "range" && sup_name == "range" => {
            let sub_lo_nexp = sub_args.first().and_then(extract_nexp);
            let sub_hi_nexp = sub_args.get(1).and_then(extract_nexp);
            let sup_lo_nexp = sup_args.first().and_then(extract_nexp);
            let sup_hi_nexp = sup_args.get(1).and_then(extract_nexp);

            match (sub_lo_nexp, sub_hi_nexp, sup_lo_nexp, sup_hi_nexp) {
                (Some(slo_e), Some(shi_e), Some(plo_e), Some(phi_e)) => {
                    // Try constant evaluation first
                    let slo_v = try_eval_const(&slo_e);
                    let shi_v = try_eval_const(&shi_e);
                    let plo_v = try_eval_const(&plo_e);
                    let phi_v = try_eval_const(&phi_e);

                    match (slo_v, shi_v, plo_v, phi_v) {
                        (Some(slo), Some(shi), Some(plo), Some(phi)) => {
                            if plo <= slo && shi <= phi {
                                SubtypeResult::Ok
                            } else {
                                SubtypeResult::Fail
                            }
                        }
                        _ => {
                            // Algebraic first, then Z3 fallback.
                            let plo_le_slo =
                                check_le(&plo_e, &slo_e).or_else(|| z3_check_le(&plo_e, &slo_e));
                            let shi_le_phi =
                                check_le(&shi_e, &phi_e).or_else(|| z3_check_le(&shi_e, &phi_e));
                            match (plo_le_slo, shi_le_phi) {
                                (Some(true), Some(true)) => SubtypeResult::Ok,
                                (Some(false), _) | (_, Some(false)) => SubtypeResult::Fail,
                                _ => {
                                    if table.raw_unify(&sub, &sup) {
                                        SubtypeResult::Ok
                                    } else {
                                        SubtypeResult::Fail
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    if table.raw_unify(&sub, &sup) {
                        SubtypeResult::Ok
                    } else {
                        SubtypeResult::Fail
                    }
                }
            }
        }

        // bits(N) <: bits(M) — requires width equality
        (
            TyKind::App { name: sub_name, args: sub_args, .. },
            TyKind::App { name: sup_name, args: sup_args, .. },
        ) if sub_name == "bits" && sup_name == "bits" => {
            let sub_w = sub_args.first().and_then(extract_nexp);
            let sup_w = sup_args.first().and_then(extract_nexp);

            match (sub_w, sup_w) {
                (Some(lhs), Some(rhs)) => {
                    match check_eq(&lhs, &rhs) {
                        Some(true) => SubtypeResult::Ok,
                        Some(false) => SubtypeResult::Fail,
                        None => {
                            // Try polynomial equivalence.
                            let lhs_poly = polynomial_from_numeric_expr(&lhs);
                            let rhs_poly = polynomial_from_numeric_expr(&rhs);
                            if let (Some(lp), Some(rp)) = (lhs_poly, rhs_poly) {
                                let diff = lp.sub(&rp);
                                if diff.as_constant() == Some(0) {
                                    return SubtypeResult::Ok;
                                }
                            }
                            // Z3 fallback for width equality.
                            if let Some(true) = z3_check_eq(&lhs, &rhs) {
                                return SubtypeResult::Ok;
                            }
                            SubtypeResult::NeedsConstraint(Obligation::WidthEquality(lhs, rhs))
                        }
                    }
                }
                _ => {
                    // Fall back to string-based comparison via raw_unify
                    if table.raw_unify(&sub, &sup) {
                        SubtypeResult::Ok
                    } else {
                        SubtypeResult::Fail
                    }
                }
            }
        }

        // Tuple subtyping: component-wise.
        (TyKind::Tuple(items1), TyKind::Tuple(items2)) if items1.len() == items2.len() => {
            for (s, p) in items1.iter().zip(items2.iter()) {
                match is_subtype(table, s, p) {
                    SubtypeResult::Ok => {}
                    other => return other,
                }
            }
            SubtypeResult::Ok
        }

        // Same-name type constructors: try unification.
        (TyKind::App { name: n1, args: a1, .. }, TyKind::App { name: n2, args: a2, .. })
            if n1 == n2 && a1.len() == a2.len() =>
        {
            // Already handled specific cases (range, bits, atom) above.
            // For other type constructors, try unification.
            if table.raw_unify(&sub, &sup) {
                SubtypeResult::Ok
            } else {
                SubtypeResult::Fail
            }
        }

        // Abstract types participate in subtyping based on Kind.
        //
        // Abstract(Kind::Int) <: int
        // Abstract(Kind::Bool) <: bool
        // Abstract(Kind::Type) <: any named type (structural compat)
        (TyKind::Abstract { kind, .. }, TyKind::Scalar(_) | TyKind::Adt(_, _)) => {
            match kind {
                Kind::Int if sup_name == Some("int") => SubtypeResult::Ok,
                Kind::Bool if sup_name == Some("bool") => SubtypeResult::Ok,
                Kind::Type => {
                    // Abstract Type kind is compatible with any named type
                    // via unification (most permissive for abstract decls)
                    if table.raw_unify(&sub, &sup) {
                        SubtypeResult::Ok
                    } else {
                        SubtypeResult::Fail
                    }
                }
                _ => SubtypeResult::Fail,
            }
        }

        // Abstract with same name: identity
        (TyKind::Abstract { name: n1, .. }, TyKind::Abstract { name: n2, .. }) if n1 == n2 => {
            SubtypeResult::Ok
        }

        // Default: fall back to symmetric unification
        _ => {
            if table.raw_unify(&sub, &sup) {
                SubtypeResult::Ok
            } else {
                SubtypeResult::Fail
            }
        }
    }
}
