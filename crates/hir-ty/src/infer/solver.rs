//! The constraint solver: does a goal follow from the assumptions?
//!
//! A layer of its own, and a small one. Everything here is a pure function
//! of `(goal, substitution, assumptions)` — it was written inside
//! `InferenceContext` and used `self` for nothing but recursion, which hid
//! that. Upstream's is a layer too (`constraint.ml`, behind `prove` in
//! `type_check.ml`); the difference is that upstream's second stage shells
//! out to Z3, which an editor cannot afford on every keystroke.
//!
//! Three answers, and `Unknown` is a real one: a checker that cannot decide
//! must say so rather than guess. rust-analyzer takes the same line with
//! const generics — it evaluates what it can and degrades otherwise.
//!
//! Structural subtype checks use `solve_without_smt` and defer undecided
//! goals until their local assumptions and inference substitutions are known.
//! The final solver stage may use the optional Z3 backend.

use super::constraint::constraint_implied_by_assumptions;
use super::numeric::eval_numeric_expr;
use super::{ConstraintStatus, Subst};
use crate::ty::{CompareOp, ConstraintExpr};

pub(super) fn solve(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    if has_erased_inputs(expr, subst, assumptions) {
        return ConstraintStatus::Unknown;
    }
    let status = solve_without_smt(expr, subst, assumptions);
    #[cfg(feature = "z3-solver")]
    let status = if matches!(status, ConstraintStatus::Unknown) {
        super::z3_solver::try_solve(expr, subst, assumptions)
    } else {
        status
    };
    status
}

/// Legacy recovery can put a primitive type name into a numeric slot. It is
/// missing value evidence, not a shared integer constant: `int - int == 0`
/// must not become a proof and `int < 0` must not produce a counterexample.
pub(super) fn has_erased_numeric(goal: &ConstraintExpr) -> bool {
    match goal {
        ConstraintExpr::Compare { lhs, rhs, .. } => {
            super::subtype::contains_erased_numeric(lhs)
                || super::subtype::contains_erased_numeric(rhs)
        }
        ConstraintExpr::InSet { value, items } => {
            super::subtype::contains_erased_numeric(value)
                || items.iter().any(super::subtype::contains_erased_numeric)
        }
        ConstraintExpr::And(xs) | ConstraintExpr::Or(xs) => xs.iter().any(has_erased_numeric),
        ConstraintExpr::Not(x) => has_erased_numeric(x),
        ConstraintExpr::App { args, .. } => args.iter().any(|a| match a {
            crate::ty::TyArg::Nexp(n) => super::subtype::contains_erased_numeric(n),
            crate::ty::TyArg::Bool(c) => has_erased_numeric(c),
            _ => false,
        }),
        _ => false,
    }
}

pub(super) fn has_erased_inputs(
    goal: &ConstraintExpr,
    subst: &Subst,
    facts: &[ConstraintExpr],
) -> bool {
    let invalid = |c: &ConstraintExpr| {
        has_erased_numeric(c)
            || ((!subst.values.entries.is_empty() || !subst.types.entries.is_empty())
                && has_erased_numeric(&super::constraint::apply_subst_constraint_expr(c, subst)))
    };
    invalid(goal) || facts.iter().any(invalid)
}

/// Cheap entailment before a goal has its full inference context.
pub(super) fn solve_without_smt(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    if has_erased_inputs(expr, subst, assumptions) {
        return ConstraintStatus::Unknown;
    }
    if super::integer::contradictory(assumptions, subst) {
        return ConstraintStatus::Satisfied;
    }
    let fast = solve_inner(expr, subst, assumptions);
    if fast != ConstraintStatus::Unknown {
        return fast;
    }
    if let Some(status) = super::boolean::decide(expr, subst, assumptions, false) {
        if status != ConstraintStatus::Unknown {
            return status;
        }
    }
    if let Some(value) = super::integer::conditional(expr, subst, assumptions) {
        return if value { ConstraintStatus::Satisfied } else { ConstraintStatus::Failed };
    }
    if let ConstraintExpr::Compare { lhs, op, rhs } = expr {
        let lhs = super::constraint::subst_numeric_expr(lhs, subst);
        let rhs = super::constraint::subst_numeric_expr(rhs, subst);
        if let Some(delta) = super::integer::difference(&lhs, &rhs) {
            return if super::integer::compare(&delta, *op, &0.into()) {
                ConstraintStatus::Satisfied
            } else {
                ConstraintStatus::Failed
            };
        }
    }
    // Exact difference constraints can entail a bound by refuting its
    // negation, including symbolic bounds such as `2 ^ 'n - 1`.
    fn entailed(goal: &ConstraintExpr, subst: &Subst, facts: &[ConstraintExpr]) -> bool {
        match goal {
            ConstraintExpr::And(xs) => xs.iter().all(|x| entailed(x, subst, facts)),
            // The graph cannot encode a disequality edge. Direct equality
            // facts and contradictory assumptions were already checked above.
            ConstraintExpr::Compare { op: CompareOp::Eq, .. } => false,
            ConstraintExpr::Compare { lhs, op, rhs } => {
                let op = match op {
                    CompareOp::Eq => CompareOp::Neq,
                    CompareOp::Neq => CompareOp::Eq,
                    CompareOp::Lt => CompareOp::Gte,
                    CompareOp::Lte => CompareOp::Gt,
                    CompareOp::Gt => CompareOp::Lte,
                    CompareOp::Gte => CompareOp::Lt,
                };
                let mut with_negation = facts.to_vec();
                with_negation.push(ConstraintExpr::Compare {
                    lhs: lhs.clone(),
                    op,
                    rhs: rhs.clone(),
                });
                super::integer::contradictory(&with_negation, subst)
            }
            _ => false,
        }
    }
    if !assumptions.is_empty() && entailed(expr, subst, assumptions) {
        return ConstraintStatus::Satisfied;
    }
    fast
}

/// A rigid elimination variable is universally quantified. A concrete
/// counterexample refutes entailment even when some other values satisfy it.
/// Flexible witnesses never enter this operation.
pub(super) fn prove_quantified(
    goal: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    rigid: &[String],
) -> ConstraintStatus {
    if has_erased_inputs(goal, subst, assumptions) {
        return ConstraintStatus::Unknown;
    }
    use crate::ty::NumericExpr;
    let status = solve_without_smt(goal, subst, assumptions);
    if status != ConstraintStatus::Unknown {
        return status;
    }
    if let Some(status) = super::boolean::decide(goal, subst, assumptions, true) {
        if status != ConstraintStatus::Unknown {
            return status;
        }
    }
    fn numbers(e: &NumericExpr, out: &mut Vec<i64>) {
        match e {
            NumericExpr::Const(n) => {
                out.push(*n);
                if let Some(n) = n.checked_sub(1) {
                    out.push(n);
                }
                if let Some(n) = n.checked_add(1) {
                    out.push(n);
                }
            }
            NumericExpr::Neg(x) | NumericExpr::Exp(x) => numbers(x, out),
            NumericExpr::Add(a, b)
            | NumericExpr::Sub(a, b)
            | NumericExpr::Mul(a, b)
            | NumericExpr::Div(a, b)
            | NumericExpr::Mod(a, b) => {
                numbers(a, out);
                numbers(b, out);
            }
            _ => {}
        }
    }
    fn constants(c: &ConstraintExpr, out: &mut Vec<i64>) {
        match c {
            ConstraintExpr::Compare { lhs, rhs, .. } => {
                numbers(lhs, out);
                numbers(rhs, out);
            }
            ConstraintExpr::InSet { value, items } => {
                numbers(value, out);
                for n in items {
                    numbers(n, out);
                }
            }
            ConstraintExpr::And(xs) | ConstraintExpr::Or(xs) => {
                for x in xs {
                    constants(x, out);
                }
            }
            ConstraintExpr::Not(x) => constants(x, out),
            _ => {}
        }
    }
    let mut candidates = vec![0, 1, -1];
    constants(goal, &mut candidates);
    for a in assumptions {
        constants(a, &mut candidates);
    }
    candidates.sort_unstable();
    candidates.dedup();
    fn search(
        goal: &ConstraintExpr,
        assumptions: &[ConstraintExpr],
        rigid: &[String],
        candidates: &[i64],
        subst: &Subst,
        budget: &mut usize,
    ) -> bool {
        if *budget == 0 {
            return false;
        }
        if let Some((name, rest)) = rigid.split_first() {
            for n in candidates {
                let mut subst = subst.clone();
                subst.values.insert(name.clone(), n.to_string());
                if search(goal, assumptions, rest, candidates, &subst, budget) {
                    return true;
                }
                if *budget == 0 {
                    break;
                }
            }
            false
        } else {
            *budget -= 1;
            assumptions
                .iter()
                .all(|a| solve_without_smt(a, subst, &[]) == ConstraintStatus::Satisfied)
                && solve_without_smt(goal, subst, assumptions) == ConstraintStatus::Failed
        }
    }
    let mut names = std::collections::HashSet::new();
    super::substitution::constraint_names(goal, &mut names);
    for a in assumptions {
        super::substitution::constraint_names(a, &mut names);
    }
    let mut universal: Vec<_> = names.into_iter().map(|n| format!("'{n}")).collect();
    universal.extend(rigid.iter().cloned());
    universal.sort();
    universal.dedup();
    universal.retain(|n| !subst.values.contains_key(n) && !subst.types.contains_key(n));
    if universal.len() <= 4 && search(goal, assumptions, &universal, &candidates, subst, &mut 128) {
        return ConstraintStatus::Failed;
    }
    #[cfg(feature = "z3-solver")]
    {
        return super::z3_solver::try_prove_quantified(goal, subst, assumptions);
    }
    #[cfg(not(feature = "z3-solver"))]
    ConstraintStatus::Unknown
}

/// `'w in {1, 2, 4, 8}` proves `'w > 0`, and refutes `'w > 8`.
///
/// The one shape of entailment worth having without an arithmetic solver,
/// and the one a constraint synonym needs: a membership in the assumptions
/// decides any comparison of that variable against a constant, because the
/// set is finite and its members are known.
fn decide_from_membership(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<ConstraintStatus> {
    let ConstraintExpr::Compare { lhs, op, rhs } = expr else { return None };
    let (var, bound, op) = match (
        eval_numeric_expr(lhs, subst, assumptions),
        eval_numeric_expr(rhs, subst, assumptions),
    ) {
        (None, Some(b)) => (subst_var_text(lhs, subst)?, b, *op),
        (Some(b), None) => {
            (subst_var_text(rhs, subst)?, b, super::constraint::swapped_compare_op(*op))
        }
        _ => return None,
    };
    let members = assumptions.iter().find_map(|a| {
        let ConstraintExpr::InSet { value, items } = a else { return None };
        if subst_var_text(value, subst).as_deref() != Some(var.as_str()) {
            return None;
        }
        items.iter().map(|i| eval_numeric_expr(i, subst, assumptions)).collect::<Option<Vec<i64>>>()
    })?;
    let holds = |m: i64| match op {
        CompareOp::Eq => m == bound,
        CompareOp::Neq => m != bound,
        CompareOp::Lt => m < bound,
        CompareOp::Lte => m <= bound,
        CompareOp::Gt => m > bound,
        CompareOp::Gte => m >= bound,
    };
    if members.iter().all(|&m| holds(m)) {
        Some(ConstraintStatus::Satisfied)
    } else if members.iter().all(|&m| !holds(m)) {
        Some(ConstraintStatus::Failed)
    } else {
        None
    }
}

/// The variable a numeric expression names after substitution, if it names one.
fn subst_var_text(expr: &crate::ty::NumericExpr, subst: &Subst) -> Option<String> {
    let text = super::constraint::subst_numeric_expr(expr, subst).to_string_repr();
    text.starts_with('\'').then_some(text)
}

/// `'n < 'd` refutes `'d < 'n`, with neither side a constant.
///
/// The two comparisons the arithmetic path cannot touch — it evaluates both
/// sides to integers or gives up, and two type variables evaluate to
/// neither. Antisymmetry needs no arithmetic: an assumption that orders the
/// same pair the other way settles the goal outright.
///
/// Without it the only thing that decided such a pair was the optional Z3
/// backend, so a build without the `z3-solver` feature — which is every
/// build the product ships — answered `Unknown` where the same query
/// answered `Failed` in a test binary that had it. Nothing states an
/// ordering it cannot use.
///
/// One assumption, no chaining: `a < b` and `b < c` do not combine here.
/// Transitivity is where an over-approximation starts, and this rule is
/// exact.
fn decide_from_ordering(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<ConstraintStatus> {
    /// `lhs op rhs` as "the smaller, the larger, is it strict", if it orders.
    fn as_ordering(
        lhs: &crate::ty::NumericExpr,
        op: CompareOp,
        rhs: &crate::ty::NumericExpr,
        subst: &Subst,
    ) -> Option<(String, String, bool)> {
        let lhs = super::constraint::subst_numeric_expr(lhs, subst).to_string_repr();
        let rhs = super::constraint::subst_numeric_expr(rhs, subst).to_string_repr();
        match op {
            CompareOp::Lt => Some((lhs, rhs, true)),
            CompareOp::Lte => Some((lhs, rhs, false)),
            CompareOp::Gt => Some((rhs, lhs, true)),
            CompareOp::Gte => Some((rhs, lhs, false)),
            CompareOp::Eq | CompareOp::Neq => None,
        }
    }

    let ConstraintExpr::Compare { lhs, op, rhs } = expr else { return None };
    let goal_lhs = super::constraint::subst_numeric_expr(lhs, subst).to_string_repr();
    let goal_rhs = super::constraint::subst_numeric_expr(rhs, subst).to_string_repr();
    if goal_lhs == goal_rhs || !goal_lhs.starts_with('\'') && !goal_rhs.starts_with('\'') {
        return None;
    }

    for assumption in assumptions {
        let ConstraintExpr::Compare { lhs, op: a_op, rhs } = assumption else { continue };
        let Some((lo, hi, strict)) = as_ordering(lhs, *a_op, rhs, subst) else { continue };
        // `lo < hi` known; say what it makes of `goal_lhs op goal_rhs`.
        let forward = lo == goal_lhs && hi == goal_rhs;
        let backward = lo == goal_rhs && hi == goal_lhs;
        if !forward && !backward {
            continue;
        }
        // Restate the goal as "is `lo` op' `hi`".
        let op = if forward { *op } else { super::constraint::swapped_compare_op(*op) };
        let status = match (op, strict) {
            // `lo < hi` gives `lo <= hi` and `lo != hi` too; `lo <= hi`
            // gives only itself.
            (CompareOp::Lt, true) | (CompareOp::Lte, _) | (CompareOp::Neq, true) => {
                ConstraintStatus::Satisfied
            }
            // `lo > hi` contradicts either form; the rest need strictness.
            (CompareOp::Gt, _) | (CompareOp::Gte, true) | (CompareOp::Eq, true) => {
                ConstraintStatus::Failed
            }
            // `lo <= hi` leaves equality open, so `lo < hi`, `lo >= hi`,
            // `lo == hi` and `lo != hi` are all still possible.
            (CompareOp::Lt, false)
            | (CompareOp::Gte, false)
            | (CompareOp::Eq, false)
            | (CompareOp::Neq, false) => continue,
        };
        return Some(status);
    }
    None
}

fn solve_inner(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    if constraint_implied_by_assumptions(assumptions, expr, subst) {
        return ConstraintStatus::Satisfied;
    }
    if let Some(status) = decide_from_membership(expr, subst, assumptions) {
        return status;
    }
    if let Some(status) = decide_from_ordering(expr, subst, assumptions) {
        return status;
    }

    match expr {
        ConstraintExpr::Bool(true) => ConstraintStatus::Satisfied,
        ConstraintExpr::Bool(false) => ConstraintStatus::Failed,
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let (Some(lhs_value), Some(rhs_value)) = (
                eval_numeric_expr(lhs, subst, assumptions),
                eval_numeric_expr(rhs, subst, assumptions),
            ) else {
                // Bounds may disprove a comparison without fixing the value:
                // 8 <= n <= 64 entails n != 128. Keep this available without SMT.
                if !assumptions.is_empty() {
                    let opposite = match op {
                        CompareOp::Eq => CompareOp::Neq,
                        CompareOp::Neq => CompareOp::Eq,
                        CompareOp::Lt => CompareOp::Gte,
                        CompareOp::Lte => CompareOp::Gt,
                        CompareOp::Gt => CompareOp::Lte,
                        CompareOp::Gte => CompareOp::Lt,
                    };
                    let negated = ConstraintExpr::Compare {
                        lhs: lhs.clone(),
                        op: opposite,
                        rhs: rhs.clone(),
                    };
                    if constraint_implied_by_assumptions(assumptions, &negated, subst) {
                        return ConstraintStatus::Failed;
                    }
                }
                return ConstraintStatus::Unknown;
            };
            let (lhs, rhs) = (lhs_value, rhs_value);
            let holds = match op {
                CompareOp::Eq => lhs == rhs,
                CompareOp::Neq => lhs != rhs,
                CompareOp::Lt => lhs < rhs,
                CompareOp::Lte => lhs <= rhs,
                CompareOp::Gt => lhs > rhs,
                CompareOp::Gte => lhs >= rhs,
            };
            if holds {
                ConstraintStatus::Satisfied
            } else {
                ConstraintStatus::Failed
            }
        }
        ConstraintExpr::InSet { value, items } => {
            if let Some(status) =
                super::bounded_arithmetic::power_membership(value, items, subst, assumptions)
            {
                return status;
            }
            let Some(value) = eval_numeric_expr(value, subst, assumptions) else {
                return ConstraintStatus::Unknown;
            };
            let mut all_known = true;
            for item in items {
                match eval_numeric_expr(item, subst, assumptions) {
                    Some(item) if item == value => return ConstraintStatus::Satisfied,
                    Some(_) => {}
                    None => all_known = false,
                }
            }
            if all_known {
                ConstraintStatus::Failed
            } else {
                ConstraintStatus::Unknown
            }
        }
        ConstraintExpr::And(items) => {
            let mut saw_unknown = false;
            for item in items {
                match solve_inner(item, subst, assumptions) {
                    ConstraintStatus::Satisfied => {}
                    ConstraintStatus::Failed => return ConstraintStatus::Failed,
                    ConstraintStatus::Unknown => saw_unknown = true,
                }
            }
            if saw_unknown {
                ConstraintStatus::Unknown
            } else {
                ConstraintStatus::Satisfied
            }
        }
        ConstraintExpr::Or(items) => {
            let mut saw_unknown = false;
            for item in items {
                match solve_inner(item, subst, assumptions) {
                    ConstraintStatus::Satisfied => return ConstraintStatus::Satisfied,
                    ConstraintStatus::Failed => {}
                    ConstraintStatus::Unknown => saw_unknown = true,
                }
            }
            if saw_unknown {
                ConstraintStatus::Unknown
            } else {
                ConstraintStatus::Failed
            }
        }
        ConstraintExpr::Not(inner) => match solve_inner(inner, subst, assumptions) {
            ConstraintStatus::Satisfied => ConstraintStatus::Failed,
            ConstraintStatus::Failed => ConstraintStatus::Satisfied,
            ConstraintStatus::Unknown => ConstraintStatus::Unknown,
        },
        ConstraintExpr::Unsupported => ConstraintStatus::Unknown,
        ConstraintExpr::App { .. } | ConstraintExpr::BoolVar(_) => ConstraintStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::solve;
    use crate::infer::{parse_constraint_text, ConstraintStatus, Subst};

    fn check(goal: &str, assumptions: &[&str]) -> ConstraintStatus {
        let assumptions = assumptions.iter().map(|t| parse_constraint_text(t)).collect::<Vec<_>>();
        solve(&parse_constraint_text(goal), &Subst::default(), &assumptions)
    }

    /// Neither side is a constant, so only antisymmetry can decide these.
    /// Every one of them answered `Unknown` in a build without Z3 — which
    /// is the build the server ships — while a test binary with the feature
    /// on answered correctly, so the tests said the reasoning was there.
    #[test]
    fn an_ordering_assumption_decides_the_reverse_ordering() {
        assert_eq!(check("'d < 'n", &["'n < 'd"]), ConstraintStatus::Failed);
        assert_eq!(check("'d <= 'n", &["'n < 'd"]), ConstraintStatus::Failed);
        assert_eq!(check("'d == 'n", &["'n < 'd"]), ConstraintStatus::Failed);
        assert_eq!(check("'n < 'd", &["'n < 'd"]), ConstraintStatus::Satisfied);
        assert_eq!(check("'n <= 'd", &["'n < 'd"]), ConstraintStatus::Satisfied);
        assert_eq!(check("'n != 'd", &["'n < 'd"]), ConstraintStatus::Satisfied);
    }

    /// `'n <= 'd` leaves equality open: it refutes `'n > 'd` and settles
    /// nothing else. A rule that answered `'n < 'd` here would be the
    /// over-approximation this one exists to avoid.
    #[test]
    fn a_non_strict_ordering_leaves_equality_open() {
        assert_eq!(check("'d < 'n", &["'n <= 'd"]), ConstraintStatus::Failed);
        assert_eq!(check("'n < 'd", &["'n <= 'd"]), ConstraintStatus::Unknown);
        assert_eq!(check("'n == 'd", &["'n <= 'd"]), ConstraintStatus::Unknown);
        assert_eq!(check("'n <= 'd", &["'n <= 'd"]), ConstraintStatus::Satisfied);
    }

    /// One assumption at a time. `'a < 'b` and `'b < 'c` do not chain in
    /// the arithmetic-free path, and saying `Unknown` is the honest answer
    /// rather than a guess. Z3 does chain them, which is what the optional
    /// feature is for — so this holds only without it.
    #[test]
    #[cfg(not(feature = "z3-solver"))]
    fn orderings_do_not_chain() {
        assert_eq!(check("'c < 'a", &["'a < 'b", "'b < 'c"]), ConstraintStatus::Unknown);
    }
}
