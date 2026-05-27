use std::collections::{HashMap, HashSet};

use super::*;

/// A polynomial in canonical form for algebraic equivalence checking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Polynomial {
    /// Map from sorted variable product (e.g. ["i", "n"]) to coefficient.
    /// An empty Vec key represents the constant term.
    terms: std::collections::BTreeMap<Vec<String>, i64>,
}

impl Polynomial {
    fn constant(c: i64) -> Self {
        let mut terms = std::collections::BTreeMap::new();
        if c != 0 {
            terms.insert(Vec::new(), c);
        }
        Self { terms }
    }

    fn variable(name: String) -> Self {
        let mut terms = std::collections::BTreeMap::new();
        terms.insert(vec![name], 1);
        Self { terms }
    }

    fn add(&self, other: &Self) -> Self {
        let mut result = self.terms.clone();
        for (k, v) in &other.terms {
            *result.entry(k.clone()).or_insert(0) += v;
        }
        result.retain(|_, v| *v != 0);
        Self { terms: result }
    }

    fn neg(&self) -> Self {
        let terms = self.terms.iter().map(|(k, v)| (k.clone(), -v)).collect();
        Self { terms }
    }

    pub(super) fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    /// Extract as a constant if this polynomial has no variable terms.
    pub(super) fn as_constant(&self) -> Option<i64> {
        if self.terms.is_empty() {
            return Some(0);
        }
        if self.terms.len() == 1 {
            if let Some((&ref key, &val)) = self.terms.iter().next() {
                if key.is_empty() {
                    return Some(val);
                }
            }
        }
        None
    }

    /// Attempt exact division by a constant. Returns `Some` only if
    /// ALL coefficients are evenly divisible by `divisor`.
    fn div_by_constant(&self, divisor: i64) -> Option<Self> {
        if divisor == 0 {
            return None;
        }
        let mut result = std::collections::BTreeMap::new();
        for (k, v) in &self.terms {
            if v % divisor != 0 {
                return None; // Not evenly divisible — give up
            }
            let quotient = v / divisor;
            if quotient != 0 {
                result.insert(k.clone(), quotient);
            }
        }
        Some(Self { terms: result })
    }

    fn mul(&self, other: &Self) -> Option<Self> {
        let mut result: std::collections::BTreeMap<Vec<String>, i64> =
            std::collections::BTreeMap::new();
        for (lk, lv) in &self.terms {
            for (rk, rv) in &other.terms {
                let mut combined = lk.clone();
                combined.extend(rk.iter().cloned());
                combined.sort();
                // Cap degree to avoid runaway expansion
                if combined.len() > 4 {
                    return None;
                }
                *result.entry(combined).or_insert(0) += lv * rv;
            }
        }
        result.retain(|_, v| *v != 0);
        Some(Self { terms: result })
    }
}

pub(super) fn polynomial_from_numeric_expr(expr: &NumericExpr) -> Option<Polynomial> {
    match expr {
        NumericExpr::Const(c) => Some(Polynomial::constant(*c)),
        NumericExpr::Var(name) | NumericExpr::Symbol(name) => {
            Some(Polynomial::variable(name.clone()))
        }
        NumericExpr::Neg(inner) => Some(polynomial_from_numeric_expr(inner)?.neg()),
        NumericExpr::Add(lhs, rhs) => {
            let l = polynomial_from_numeric_expr(lhs)?;
            let r = polynomial_from_numeric_expr(rhs)?;
            Some(l.add(&r))
        }
        NumericExpr::Sub(lhs, rhs) => {
            let l = polynomial_from_numeric_expr(lhs)?;
            let r = polynomial_from_numeric_expr(rhs)?;
            Some(l.sub(&r))
        }
        NumericExpr::Mul(lhs, rhs) => {
            let l = polynomial_from_numeric_expr(lhs)?;
            let r = polynomial_from_numeric_expr(rhs)?;
            l.mul(&r)
        }
        // Division, modulo, exponentiation aren't representable in
        // polynomial form in general. However:
        //
        // 1. Both constants → fold immediately
        // 2. Divisor is constant → attempt per-term division (if exact)
        // 3. Otherwise → return None (delegate to Z3)
        NumericExpr::Div(lhs, rhs) => {
            let l = polynomial_from_numeric_expr(lhs)?;
            let r = polynomial_from_numeric_expr(rhs)?;
            // Case 1: Both constants.
            if let (Some(lc), Some(rc)) = (l.as_constant(), r.as_constant()) {
                if rc == 0 {
                    return None;
                }
                return Some(Polynomial::constant(lc / rc));
            }
            // Case 2: Divisor is constant — try exact per-term division.
            if let Some(divisor) = r.as_constant() {
                if divisor == 0 {
                    return None;
                }
                return l.div_by_constant(divisor);
            }
            // Case 3: Not polynomial-representable.
            None
        }
        NumericExpr::Mod(lhs, rhs) => {
            let l = polynomial_from_numeric_expr(lhs)?;
            let r = polynomial_from_numeric_expr(rhs)?;
            // Case 1: Both constants.
            if let (Some(lc), Some(rc)) = (l.as_constant(), r.as_constant()) {
                if rc == 0 {
                    return None;
                }
                return Some(Polynomial::constant(lc % rc));
            }
            // Case 2: Numerator is constant, divisor is constant.
            // (Mod of symbolic by constant is not polynomial — delegate to Z3.)
            if let Some(rc) = r.as_constant() {
                if rc == 0 {
                    return None;
                }
                if let Some(lc) = l.as_constant() {
                    return Some(Polynomial::constant(lc % rc));
                }
            }
            // Case 3: Not polynomial-representable.
            None
        }
        NumericExpr::Exp(inner) => {
            // Exponentiation: 2^n (Sail convention: base is always 2).
            // Exponentiation: 2^n (base is always 2).
            //
            // Case 1: inner is a literal constant.
            if let NumericExpr::Const(n) = inner.as_ref() {
                if *n >= 0 && *n <= 63 {
                    return Some(Polynomial::constant(1i64 << n));
                }
            }
            // Case 2: inner evaluates to a constant via polynomial algebra.
            let inner_poly = polynomial_from_numeric_expr(inner)?;
            let n = inner_poly.as_constant()?;
            if n >= 0 && n <= 63 {
                Some(Polynomial::constant(1i64 << n))
            } else {
                None // Overflow or negative exponent — delegate to Z3
            }
        }
        // If-then-else is not representable in polynomial form.
        NumericExpr::App { .. } => None, // not polynomial
        NumericExpr::If { .. } => None,
    }
}

pub(super) fn swapped_compare_op(op: CompareOp) -> CompareOp {
    match op {
        CompareOp::Eq => CompareOp::Eq,
        CompareOp::Neq => CompareOp::Neq,
        CompareOp::Lt => CompareOp::Gt,
        CompareOp::Lte => CompareOp::Gte,
        CompareOp::Gt => CompareOp::Lt,
        CompareOp::Gte => CompareOp::Lte,
    }
}

fn numeric_expr_key(expr: &NumericExpr) -> String {
    match expr {
        NumericExpr::Const(value) => value.to_string(),
        NumericExpr::Var(name) | NumericExpr::Symbol(name) => name.clone(),
        NumericExpr::Neg(inner) => format!("(-{})", numeric_expr_key(inner)),
        NumericExpr::Add(lhs, rhs) => {
            format!("({}+{})", numeric_expr_key(lhs), numeric_expr_key(rhs))
        }
        NumericExpr::Sub(lhs, rhs) => {
            format!("({}-{})", numeric_expr_key(lhs), numeric_expr_key(rhs))
        }
        NumericExpr::Mul(lhs, rhs) => {
            format!("({}*{})", numeric_expr_key(lhs), numeric_expr_key(rhs))
        }
        NumericExpr::Div(lhs, rhs) => {
            format!("({}/{})", numeric_expr_key(lhs), numeric_expr_key(rhs))
        }
        NumericExpr::Mod(lhs, rhs) => {
            format!("({}%{})", numeric_expr_key(lhs), numeric_expr_key(rhs))
        }
        NumericExpr::Exp(inner) => {
            format!("(2^{})", numeric_expr_key(inner))
        }
        NumericExpr::App { name, args } => {
            let args_str =
                args.iter().map(numeric_expr_key).collect::<Vec<_>>().join(",");
            format!("{}({})", name, args_str)
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
            format!(
                "(if {} then {} else {})",
                cond.to_text(),
                numeric_expr_key(then_expr),
                numeric_expr_key(else_expr),
            )
        }
    }
}

pub(super) fn subst_numeric_expr(expr: &NumericExpr, subst: &Subst) -> NumericExpr {
    match expr {
        NumericExpr::Const(value) => NumericExpr::Const(*value),
        NumericExpr::Var(name) => {
            let resolved =
                subst.values.get(name).and_then(|value| parse_numeric_expr_text(value)).or_else(
                    || {
                        subst.types.get(name).and_then(|ty| {
                            let text = ty.display_text();
                            parse_numeric_expr_text(&text)
                        })
                    },
                );
            match resolved {
                Some(NumericExpr::Var(bound)) if &bound == name => NumericExpr::Var(name.clone()),
                Some(expr) => subst_numeric_expr(&expr, subst),
                None => NumericExpr::Var(name.clone()),
            }
        }
        NumericExpr::Symbol(name) => NumericExpr::Symbol(name.clone()),
        NumericExpr::Neg(inner) => NumericExpr::Neg(Box::new(subst_numeric_expr(inner, subst))),
        NumericExpr::Add(lhs, rhs) => NumericExpr::Add(
            Box::new(subst_numeric_expr(lhs, subst)),
            Box::new(subst_numeric_expr(rhs, subst)),
        ),
        NumericExpr::Sub(lhs, rhs) => NumericExpr::Sub(
            Box::new(subst_numeric_expr(lhs, subst)),
            Box::new(subst_numeric_expr(rhs, subst)),
        ),
        NumericExpr::Mul(lhs, rhs) => NumericExpr::Mul(
            Box::new(subst_numeric_expr(lhs, subst)),
            Box::new(subst_numeric_expr(rhs, subst)),
        ),
        NumericExpr::Div(lhs, rhs) => NumericExpr::Div(
            Box::new(subst_numeric_expr(lhs, subst)),
            Box::new(subst_numeric_expr(rhs, subst)),
        ),
        NumericExpr::Mod(lhs, rhs) => NumericExpr::Mod(
            Box::new(subst_numeric_expr(lhs, subst)),
            Box::new(subst_numeric_expr(rhs, subst)),
        ),
        NumericExpr::Exp(inner) => NumericExpr::Exp(Box::new(subst_numeric_expr(inner, subst))),
        NumericExpr::App { name, args } => NumericExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| subst_numeric_expr(a, subst)).collect(),
        },
        NumericExpr::If { cond, then_expr, else_expr } => {
            NumericExpr::If {
                cond: Box::new(apply_subst_constraint_expr(cond, subst)),
                then_expr: Box::new(subst_numeric_expr(then_expr, subst)),
                else_expr: Box::new(subst_numeric_expr(else_expr, subst)),
            }
        }
    }
}

fn stronger_lower_bound(current: NumericBound, candidate: NumericBound) -> NumericBound {
    if candidate.value > current.value
        || (candidate.value == current.value && !candidate.inclusive && current.inclusive)
    {
        candidate
    } else {
        current
    }
}

fn stronger_upper_bound(current: NumericBound, candidate: NumericBound) -> NumericBound {
    if candidate.value < current.value
        || (candidate.value == current.value && !candidate.inclusive && current.inclusive)
    {
        candidate
    } else {
        current
    }
}

fn constraint_facts_add_compare(facts: &mut ConstraintFacts, op: CompareOp, value: i64) {
    match op {
        CompareOp::Eq => {
            let singleton = HashSet::from([value]);
            facts.exact_values = Some(match facts.exact_values.take() {
                Some(values) => values.intersection(&singleton).copied().collect(),
                None => singleton,
            });
            facts.lower = Some(match facts.lower {
                Some(current) => {
                    stronger_lower_bound(current, NumericBound { value, inclusive: true })
                }
                None => NumericBound { value, inclusive: true },
            });
            facts.upper = Some(match facts.upper {
                Some(current) => {
                    stronger_upper_bound(current, NumericBound { value, inclusive: true })
                }
                None => NumericBound { value, inclusive: true },
            });
        }
        CompareOp::Neq => {
            facts.excluded_values.insert(value);
        }
        CompareOp::Gt => {
            let bound = NumericBound { value, inclusive: false };
            facts.lower = Some(match facts.lower {
                Some(current) => stronger_lower_bound(current, bound),
                None => bound,
            });
        }
        CompareOp::Gte => {
            let bound = NumericBound { value, inclusive: true };
            facts.lower = Some(match facts.lower {
                Some(current) => stronger_lower_bound(current, bound),
                None => bound,
            });
        }
        CompareOp::Lt => {
            let bound = NumericBound { value, inclusive: false };
            facts.upper = Some(match facts.upper {
                Some(current) => stronger_upper_bound(current, bound),
                None => bound,
            });
        }
        CompareOp::Lte => {
            let bound = NumericBound { value, inclusive: true };
            facts.upper = Some(match facts.upper {
                Some(current) => stronger_upper_bound(current, bound),
                None => bound,
            });
        }
    }
}

fn constraint_facts_add_set(facts: &mut ConstraintFacts, values: HashSet<i64>) {
    facts.exact_values = Some(match facts.exact_values.take() {
        Some(existing) => existing.intersection(&values).copied().collect(),
        None => values,
    });
}

fn bound_min(bound: NumericBound) -> i64 {
    if bound.inclusive {
        bound.value
    } else {
        bound.value.saturating_add(1)
    }
}

fn bound_max(bound: NumericBound) -> i64 {
    if bound.inclusive {
        bound.value
    } else {
        bound.value.saturating_sub(1)
    }
}

fn facts_possible_values(facts: &ConstraintFacts) -> Option<HashSet<i64>> {
    let mut values = facts.exact_values.clone()?;
    if let Some(lower) = facts.lower {
        let lower = bound_min(lower);
        values.retain(|value| *value >= lower);
    }
    if let Some(upper) = facts.upper {
        let upper = bound_max(upper);
        values.retain(|value| *value <= upper);
    }
    values.retain(|value| !facts.excluded_values.contains(value));
    Some(values)
}

fn facts_are_contradictory(facts: &ConstraintFacts) -> bool {
    if let Some(values) = facts_possible_values(facts) {
        return values.is_empty();
    }

    if let (Some(lower), Some(upper)) = (facts.lower, facts.upper) {
        let lower = bound_min(lower);
        let upper = bound_max(upper);
        if lower > upper {
            return true;
        }
        if lower == upper && facts.excluded_values.contains(&lower) {
            return true;
        }
    }

    false
}

fn compare_holds(lhs: i64, op: CompareOp, rhs: i64) -> bool {
    match op {
        CompareOp::Eq => lhs == rhs,
        CompareOp::Neq => lhs != rhs,
        CompareOp::Lt => lhs < rhs,
        CompareOp::Lte => lhs <= rhs,
        CompareOp::Gt => lhs > rhs,
        CompareOp::Gte => lhs >= rhs,
    }
}

fn facts_imply_compare(facts: &ConstraintFacts, op: CompareOp, target: i64) -> bool {
    if let Some(values) = facts_possible_values(facts) {
        return !values.is_empty() && values.iter().all(|value| compare_holds(*value, op, target));
    }

    match op {
        CompareOp::Eq => {
            if let (Some(lower), Some(upper)) = (facts.lower, facts.upper) {
                let lower = bound_min(lower);
                let upper = bound_max(upper);
                lower == target && upper == target && !facts.excluded_values.contains(&target)
            } else {
                false
            }
        }
        CompareOp::Neq => facts
            .lower
            .map(bound_min)
            .zip(facts.upper.map(bound_max))
            .map(|(lower, upper)| target < lower || target > upper)
            .unwrap_or_else(|| facts.excluded_values.contains(&target)),
        CompareOp::Gt => facts.lower.map(bound_min).is_some_and(|lower| lower > target),
        CompareOp::Gte => facts.lower.map(bound_min).is_some_and(|lower| lower >= target),
        CompareOp::Lt => facts.upper.map(bound_max).is_some_and(|upper| upper < target),
        CompareOp::Lte => facts.upper.map(bound_max).is_some_and(|upper| upper <= target),
    }
}

fn direct_constraint_match(
    assumption: &ConstraintExpr,
    target: &ConstraintExpr,
    subst: &Subst,
) -> bool {
    fn compare_matches(
        lhs: &NumericExpr,
        op: CompareOp,
        rhs: &NumericExpr,
        other_lhs: &NumericExpr,
        other_op: CompareOp,
        other_rhs: &NumericExpr,
        subst: &Subst,
    ) -> bool {
        let lhs = subst_numeric_expr(lhs, subst);
        let rhs = subst_numeric_expr(rhs, subst);
        let other_lhs = subst_numeric_expr(other_lhs, subst);
        let other_rhs = subst_numeric_expr(other_rhs, subst);

        (op == other_op && lhs == other_lhs && rhs == other_rhs)
            || (swapped_compare_op(op) == other_op && lhs == other_rhs && rhs == other_lhs)
    }

    match (assumption, target) {
        (ConstraintExpr::Bool(lhs), ConstraintExpr::Bool(rhs)) => lhs == rhs,
        (
            ConstraintExpr::Compare { lhs, op, rhs },
            ConstraintExpr::Compare { lhs: other_lhs, op: other_op, rhs: other_rhs },
        ) => compare_matches(lhs, *op, rhs, other_lhs, *other_op, other_rhs, subst),
        (
            ConstraintExpr::InSet { value, items },
            ConstraintExpr::InSet { value: other_value, items: other_items },
        ) => {
            subst_numeric_expr(value, subst) == subst_numeric_expr(other_value, subst)
                && items.len() == other_items.len()
                && items.iter().zip(other_items.iter()).all(|(lhs, rhs)| {
                    subst_numeric_expr(lhs, subst) == subst_numeric_expr(rhs, subst)
                })
        }
        (
            ConstraintExpr::Compare { lhs, op: CompareOp::Eq, rhs },
            ConstraintExpr::InSet { value, items },
        )
        | (
            ConstraintExpr::InSet { value, items },
            ConstraintExpr::Compare { lhs, op: CompareOp::Eq, rhs },
        ) if items.len() == 1 => {
            let lhs = subst_numeric_expr(lhs, subst);
            let rhs = subst_numeric_expr(rhs, subst);
            let value = subst_numeric_expr(value, subst);
            let item = subst_numeric_expr(&items[0], subst);
            (lhs == value && rhs == item) || (lhs == item && rhs == value)
        }
        (ConstraintExpr::Not(lhs), ConstraintExpr::Not(rhs)) => {
            direct_constraint_match(lhs, rhs, subst)
        }
        _ => false,
    }
}

fn collect_constraint_facts(
    constraint: &ConstraintExpr,
    subst: &Subst,
    facts: &mut HashMap<String, ConstraintFacts>,
) -> bool {
    match constraint {
        ConstraintExpr::Bool(true) => false,
        ConstraintExpr::Bool(false) => true,
        ConstraintExpr::And(items) => {
            items.iter().any(|item| collect_constraint_facts(item, subst, facts))
        }
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs = subst_numeric_expr(lhs, subst);
            let rhs = subst_numeric_expr(rhs, subst);
            let lhs_const = eval_numeric_expr(&lhs, subst, &[]);
            let rhs_const = eval_numeric_expr(&rhs, subst, &[]);

            match (lhs_const, rhs_const) {
                (Some(lhs), Some(rhs)) => !compare_holds(lhs, *op, rhs),
                (_, Some(value)) => {
                    let key = numeric_expr_key(&lhs);
                    let entry = facts.entry(key).or_default();
                    constraint_facts_add_compare(entry, *op, value);
                    facts_are_contradictory(entry)
                }
                (Some(value), _) => {
                    let key = numeric_expr_key(&rhs);
                    let entry = facts.entry(key).or_default();
                    constraint_facts_add_compare(entry, swapped_compare_op(*op), value);
                    facts_are_contradictory(entry)
                }
                (None, None) => false,
            }
        }
        ConstraintExpr::InSet { value, items } => {
            let value = subst_numeric_expr(value, subst);
            let item_values = items
                .iter()
                .map(|item| eval_numeric_expr(&subst_numeric_expr(item, subst), subst, &[]))
                .collect::<Option<Vec<_>>>();

            let Some(item_values) = item_values else {
                return false;
            };

            if let Some(value) = eval_numeric_expr(&value, subst, &[]) {
                !item_values.contains(&value)
            } else {
                let key = numeric_expr_key(&value);
                let entry = facts.entry(key).or_default();
                constraint_facts_add_set(entry, item_values.into_iter().collect());
                facts_are_contradictory(entry)
            }
        }
        ConstraintExpr::Or(_)
        | ConstraintExpr::Not(_)
        | ConstraintExpr::Unsupported
        | ConstraintExpr::App { .. }
        | ConstraintExpr::BoolVar(_) => false,
    }
}

fn constraint_implied_by_facts(
    target: &ConstraintExpr,
    subst: &Subst,
    facts: &HashMap<String, ConstraintFacts>,
) -> bool {
    match target {
        ConstraintExpr::Bool(true) => true,
        ConstraintExpr::Bool(false) => false,
        ConstraintExpr::And(items) => {
            items.iter().all(|item| constraint_implied_by_facts(item, subst, facts))
        }
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs = subst_numeric_expr(lhs, subst);
            let rhs = subst_numeric_expr(rhs, subst);
            match (
                facts.get(&numeric_expr_key(&lhs)),
                eval_numeric_expr(&rhs, subst, &[]),
                eval_numeric_expr(&lhs, subst, &[]),
                facts.get(&numeric_expr_key(&rhs)),
            ) {
                (Some(facts), Some(value), _, _) => facts_imply_compare(facts, *op, value),
                (_, _, Some(value), Some(facts)) => {
                    facts_imply_compare(facts, swapped_compare_op(*op), value)
                }
                _ => false,
            }
        }
        ConstraintExpr::InSet { value, items } => {
            let value = subst_numeric_expr(value, subst);
            let target_values = items
                .iter()
                .map(|item| eval_numeric_expr(&subst_numeric_expr(item, subst), subst, &[]))
                .collect::<Option<HashSet<_>>>();
            let Some(target_values) = target_values else {
                return false;
            };
            facts.get(&numeric_expr_key(&value)).is_some_and(|facts| {
                facts_possible_values(facts)
                    .map(|values| !values.is_empty() && values.is_subset(&target_values))
                    .unwrap_or(false)
            })
        }
        ConstraintExpr::Not(_)
        | ConstraintExpr::Or(_)
        | ConstraintExpr::Unsupported
        | ConstraintExpr::App { .. }
        | ConstraintExpr::BoolVar(_) => false,
    }
}

pub(super) fn constraint_implied_by_assumptions(
    assumptions: &[ConstraintExpr],
    target: &ConstraintExpr,
    subst: &Subst,
) -> bool {
    if assumptions.iter().any(|assumption| direct_constraint_match(assumption, target, subst)) {
        return true;
    }

    let mut facts = HashMap::new();
    for assumption in assumptions {
        collect_constraint_facts(assumption, subst, &mut facts);
    }
    constraint_implied_by_facts(target, subst, &facts)
}

pub(super) fn unify_value(expected: &str, actual: &str, subst: &mut Subst) -> bool {
    if normalized_value_text(expected) == normalized_value_text(actual) {
        return true;
    }

    if expected.starts_with('\'') {
        match subst.values.get(expected) {
            Some(bound) => {
                let bound = bound.clone();
                bound == actual || unify_value(&bound, actual, subst)
            }
            None => {
                subst.values.insert(expected.to_string(), actual.to_string());
                true
            }
        }
    } else if actual.starts_with('\'') {
        // Symmetric: if actual is a type variable, try to bind it.
        match subst.values.get(actual) {
            Some(bound) => {
                let bound = bound.clone();
                bound == expected || unify_value(expected, &bound, subst)
            }
            None => {
                subst.values.insert(actual.to_string(), expected.to_string());
                true
            }
        }
    } else if let (Some(expected), Some(actual)) =
        (parse_numeric_expr_text(expected), parse_numeric_expr_text(actual))
    {
        unify_numeric_expr(&expected, &actual, subst)
    } else {
        normalized_value_text(expected) == normalized_value_text(actual)
    }
}

pub(super) fn apply_subst(ty: &Ty, subst: &Subst) -> Ty {
    match ty.kind() {
        TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)) => ty.clone(),
        TyKind::Scalar(_) | TyKind::Adt(_, _) => ty.clone(),
        TyKind::Param(name) => subst.types.get(name).cloned().unwrap_or_else(|| ty.clone()),
        TyKind::Tuple(items) => {
            Ty::tuple(items.iter().map(|item| apply_subst(item, subst)).collect())
        }
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => Ty::function(
            params.iter().map(|param| apply_subst(param, subst)).collect(),
            apply_subst(ret, subst),
        ),
        TyKind::App { name, args, .. } => {
            let args = args
                .iter()
                .map(|arg| match arg {
                    TyArg::Type(t) => TyArg::Type(apply_subst(t, subst)),
                    TyArg::Nexp(nexp) => {
                        // Convert to string, substitute, then re-parse
                        let value = nexp.to_string_repr();
                        let substituted = if let Some(v) = subst.values.get(&value) {
                            v.clone()
                        } else {
                            let mut result = value.clone();
                            for (from, to) in subst.values.entries.iter() {
                                if result.contains(from.as_str()) {
                                    result = result.replace(from.as_str(), to);
                                }
                            }
                            result
                        };
                        TyArg::numeric(substituted)
                    }
                    TyArg::Value(value) => {
                        // First try exact match, then substring replacement
                        // for expressions like "2 * 'n" → "2 * 'n#0".
                        // Fresh vars replace occurrences within numeric expressions.
                        let substituted = if let Some(v) = subst.values.get(value) {
                            v.clone()
                        } else {
                            let mut result = value.clone();
                            for (from, to) in subst.values.entries.iter() {
                                if result.contains(from.as_str()) {
                                    result = result.replace(from.as_str(), to);
                                }
                            }
                            result
                        };
                        TyArg::Value(substituted)
                    }
                })
                .collect::<Vec<_>>();
            let text = app_text(name, &args);
            Ty::app(name.clone(), args, text)
        }
        TyKind::Exist { vars, constraint, inner } => {
            Ty::exist(vars.clone(), constraint.clone(), apply_subst(inner, subst))
        }
        TyKind::Bidir { lhs, rhs } => Ty::bidir(apply_subst(lhs, subst), apply_subst(rhs, subst)),
        TyKind::Abstract { .. } => ty.clone(), // opaque — no substitution
    }
}

/// Apply a `Subst`'s value mapping to a `NumericExpr`, renaming `Var` nodes.
pub(super) fn apply_subst_numeric(expr: &NumericExpr, subst: &Subst) -> NumericExpr {
    match expr {
        NumericExpr::Const(c) => NumericExpr::Const(*c),
        NumericExpr::Var(name) => {
            if let Some(replacement) = subst.values.get(name) {
                NumericExpr::Var(replacement.clone())
            } else {
                NumericExpr::Var(name.clone())
            }
        }
        NumericExpr::Symbol(s) => NumericExpr::Symbol(s.clone()),
        NumericExpr::Neg(inner) => NumericExpr::Neg(Box::new(apply_subst_numeric(inner, subst))),
        NumericExpr::Add(l, r) => NumericExpr::Add(
            Box::new(apply_subst_numeric(l, subst)),
            Box::new(apply_subst_numeric(r, subst)),
        ),
        NumericExpr::Sub(l, r) => NumericExpr::Sub(
            Box::new(apply_subst_numeric(l, subst)),
            Box::new(apply_subst_numeric(r, subst)),
        ),
        NumericExpr::Mul(l, r) => NumericExpr::Mul(
            Box::new(apply_subst_numeric(l, subst)),
            Box::new(apply_subst_numeric(r, subst)),
        ),
        NumericExpr::Div(l, r) => NumericExpr::Div(
            Box::new(apply_subst_numeric(l, subst)),
            Box::new(apply_subst_numeric(r, subst)),
        ),
        NumericExpr::Mod(l, r) => NumericExpr::Mod(
            Box::new(apply_subst_numeric(l, subst)),
            Box::new(apply_subst_numeric(r, subst)),
        ),
        NumericExpr::Exp(inner) => NumericExpr::Exp(Box::new(apply_subst_numeric(inner, subst))),
        NumericExpr::App { name, args } => NumericExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| apply_subst_numeric(a, subst)).collect(),
        },
        NumericExpr::If { cond, then_expr, else_expr } => NumericExpr::If {
            cond: Box::new(apply_subst_constraint_expr(cond, subst)),
            then_expr: Box::new(apply_subst_numeric(then_expr, subst)),
            else_expr: Box::new(apply_subst_numeric(else_expr, subst)),
        },
    }
}

/// Apply a `Subst`'s value mapping to a `ConstraintExpr`, renaming variables.
pub(super) fn apply_subst_constraint_expr(expr: &ConstraintExpr, subst: &Subst) -> ConstraintExpr {
    match expr {
        ConstraintExpr::Bool(b) => ConstraintExpr::Bool(*b),
        ConstraintExpr::Compare { lhs, op, rhs } => ConstraintExpr::Compare {
            lhs: apply_subst_numeric(lhs, subst),
            op: *op,
            rhs: apply_subst_numeric(rhs, subst),
        },
        ConstraintExpr::InSet { value, items } => ConstraintExpr::InSet {
            value: apply_subst_numeric(value, subst),
            items: items.iter().map(|i| apply_subst_numeric(i, subst)).collect(),
        },
        ConstraintExpr::And(parts) => ConstraintExpr::And(
            parts.iter().map(|p| apply_subst_constraint_expr(p, subst)).collect(),
        ),
        ConstraintExpr::Or(parts) => ConstraintExpr::Or(
            parts.iter().map(|p| apply_subst_constraint_expr(p, subst)).collect(),
        ),
        ConstraintExpr::Not(inner) => {
            ConstraintExpr::Not(Box::new(apply_subst_constraint_expr(inner, subst)))
        }
        ConstraintExpr::Unsupported => ConstraintExpr::Unsupported,
        ConstraintExpr::App { name, args } => ConstraintExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| apply_subst_constraint_expr(a, subst)).collect(),
        },
        ConstraintExpr::BoolVar(v) => {
            // Check if there's a substitution for this boolean variable
            ConstraintExpr::BoolVar(v.clone())
        }
    }
}

/// Whether a textual type name is a Sail numeric scalar type.
pub(super) fn is_numeric_text(t: &str) -> bool {
    t == "int"
        || t == "nat"
        || t.starts_with("range(")
        || t.starts_with("atom(")
        || t.starts_with("int(")
        || t.starts_with("nat(")
}

/// Whether `ty` is a Sail numeric scalar type (allocation-free).
pub(super) fn is_numeric_scalar_ty(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Scalar(s) => matches!(s, crate::ty::Scalar::Int | crate::ty::Scalar::Nat),
        TyKind::Adt(t, _) => t == "int" || t == "nat",
        TyKind::App { name, .. } => {
            matches!(name.as_str(), "range" | "atom" | "int" | "nat")
        }
        _ => false,
    }
}

/// Whether `ty` is a primitive scalar or bits type.
pub(super) fn is_primitive_or_bits_ty(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Adt(t, _) => {
            matches!(t.as_str(), "int" | "nat" | "bool" | "string" | "unit" | "real" | "bit" | "_")
        }
        TyKind::App { name, .. } => {
            matches!(name.as_str(), "bits" | "range" | "atom" | "int" | "nat")
        }
        _ => false,
    }
}

/// Extract bitvector width from `bits(N)` or `bit`.
pub(super) fn bits_width(ty: &Ty) -> Option<String> {
    match ty.kind() {
        TyKind::Scalar(crate::ty::Scalar::Bit) => Some("1".to_string()),
        TyKind::App { name, args, .. } if name == "bits" => {
            args.first().and_then(|a| a.as_value_str())
        }
        _ => None,
    }
}

const UNIFY_DEPTH_LIMIT: usize = 96;

pub(super) fn unify(expected: &Ty, actual: &Ty, subst: &mut Subst) -> bool {
    unify_inner(expected, actual, subst, 0)
}

fn unify_inner(expected: &Ty, actual: &Ty, subst: &mut Subst, depth: usize) -> bool {
    if depth > UNIFY_DEPTH_LIMIT {
        // Refuse to recurse further. Conservatively accept — the
        // alternative is reporting a spurious mismatch on a deeply
        // nested type we couldn't fully walk.
        return true;
    }
    // Symmetric early-out for Unknown: either side being Unknown means we
    // can't verify types (typically a cross-file reference), so we accept.
    if actual.is_error() {
        return true;
    }
    // + Existential on actual side — unwrap with witness extraction.
    //
    // When `actual` is `{'n, constraint. inner}`, we unify `expected` with
    // `inner` and verify the constraint holds with extracted witnesses.
    // If witness extraction fails (symbolic vars remain), fall back to
    // simple unwrap (optimistic: LSP shouldn't produce false positives).
    if let TyKind::Exist { vars, constraint, inner } = actual.kind() {
        // First: try to unify structurally (the inner type must match expected)
        let ok = unify_inner(expected, inner, subst, depth + 1);
        if ok && !vars.is_empty() {
            // Attempt witness extraction and constraint verification
            // using 's existential module.
            use super::existential;
            let mut table = super::InferenceTable::default();
            match existential::extract_witnesses(vars, constraint, inner, expected, &mut table) {
                existential::WitnessResult::ConstraintViolation { .. } => {
                    // The constraint is provably violated — unification fails
                    return false;
                }
                _ => {
                    // Ok or Deferred — accept the unification
                }
            }
        }
        return ok;
    }
    match expected.kind() {
        TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)) => true,
        TyKind::Param(name) => {
            if matches!(actual.kind(), TyKind::Param(actual_name) if actual_name == name) {
                return true;
            }
            if ty_contains_var(actual, name) {
                return false;
            }
            match subst.types.get(name).cloned() {
                Some(bound) => unify_inner(&bound, actual, subst, depth + 1),
                None => {
                    subst.types.insert(name.clone(), actual.clone());
                    true
                }
            }
        }
        TyKind::Scalar(expected_scalar) => {
            // Scalar-to-scalar: must be the same scalar.
            if let TyKind::Scalar(actual_scalar) = actual.kind() {
                return expected_scalar == actual_scalar;
            }
            // bit ↔ bits(1) equivalence.
            // Scalar::Bit expected vs App("bits", ["1"]) actual.
            if *expected_scalar == crate::ty::Scalar::Bit {
                if let Some(width) = bits_width(actual) {
                    if width.trim() == "1" {
                        return true;
                    }
                }
            }
            // Scalar-to-Adt: check textual match (e.g., "int" from type alias).
            if let Some(actual_name) = actual.as_name() {
                return expected_scalar.name() == actual_name;
            }
            return false;
        }
        TyKind::Adt(expected, _) => {
            if matches!(actual.kind(), TyKind::Tuple(_) | TyKind::FnPtr(..)) {
                return false;
            }
            // Direct match: Adt-to-Adt, or Adt-to-Scalar.
            if let Some(actual_name) = actual.as_name() {
                if expected == actual_name {
                    return true;
                }
            }
            if let TyKind::Adt(actual_text, _) = actual.kind() {
                if expected == actual_text {
                    return true;
                }
                // bit ≡ bits(1) — check via the textual form on the left
                // (the App-side analogue is handled lower down).
                if (expected == "bit" && actual_text == "bits(1)")
                    || (expected == "bits(1)" && actual_text == "bit")
                {
                    return true;
                }
                // Two different primitives are not equivalent unless they
                // are both numeric scalars (which fall into the structural
                // numeric check below).
                let primitives = ["int", "nat", "bool", "string", "unit", "real", "bit", "_"];
                let exp_prim = primitives.contains(&expected.as_str());
                let act_prim = primitives.contains(&actual_text.as_str());
                if exp_prim
                    && act_prim
                    && !(is_numeric_text(expected) && is_numeric_text(actual_text))
                {
                    return false;
                }
            }
            // Sail subtyping: int ↔ range(...) ↔ atom(...) ↔ nat ↔ int(N)
            // are all numeric and the LSP can't verify exact constraints.
            if is_numeric_text(expected) && is_numeric_scalar_ty(actual) {
                return true;
            }
            // bit ≡ bits(1) (App form on the right).
            if expected == "bit" {
                if let Some(width) = bits_width(actual) {
                    return width.trim() == "1";
                }
            }
            // bits(N) form on the LHS string vs structured `Ty::App` on RHS.
            if expected.starts_with("bits(") && expected.ends_with(')') {
                let expected_width = &expected["bits(".len()..expected.len() - 1];
                if let Some(actual_width) = bits_width(actual) {
                    let exp_num = expected_width.parse::<i64>().ok();
                    let act_num = actual_width.parse::<i64>().ok();
                    return match (exp_num, act_num) {
                        (Some(a), Some(b)) => a == b,
                        // Either side has a type variable / arithmetic — we
                        // can't decide without SMT, so be permissive.
                        _ => true,
                    };
                }
            }
            // Cross-file type aliases (e.g. xlenbits = bits(64)) are
            // unknown to the local type checker. If either side is a non-
            // primitive type name we can't resolve, treat them as compatible.
            let primitives = ["int", "nat", "bool", "string", "unit", "real", "bit", "_"];
            let is_expected_primitive = primitives.contains(&expected.as_str());
            let is_actual_primitive = is_primitive_or_bits_ty(actual);
            if !is_expected_primitive || !is_actual_primitive {
                return true;
            }
            false
        }
        TyKind::Tuple(expected_items) => match actual.kind() {
            TyKind::Tuple(actual_items) if expected_items.len() == actual_items.len() => {
                expected_items
                    .iter()
                    .zip(actual_items.iter())
                    .all(|(e, a)| unify_inner(e, a, subst, depth + 1))
            }
            _ => false,
        },
        TyKind::FnPtr(crate::ty::FnSig { params: expected_params, ret: expected_ret }) => {
            match actual.kind() {
                TyKind::FnPtr(crate::ty::FnSig { params: actual_params, ret: actual_ret })
                    if expected_params.len() == actual_params.len() =>
                {
                    expected_params
                        .iter()
                        .zip(actual_params.iter())
                        .all(|(e, a)| unify_inner(e, a, subst, depth + 1))
                        && unify_inner(expected_ret, actual_ret, subst, depth + 1)
                }
                _ => false,
            }
        }
        TyKind::App { name: expected_name, args: expected_args, .. } => {
            // Sail subtyping: range/atom/nat/int and parameterized int(N) /
            // atom(N) / nat(N) are all numeric. Structural check, no
            // string allocation.
            if matches!(expected_name.as_str(), "range" | "atom" | "int" | "nat")
                && is_numeric_scalar_ty(actual)
            {
                return true;
            }
            // bit ≡ bits(1) — accept either direction.
            if expected_name == "bits"
                && matches!(actual.kind(), TyKind::Scalar(crate::ty::Scalar::Bit))
            {
                if let Some(width) = expected_args.first().and_then(|a| a.as_value_str()) {
                    if width.trim() == "1" {
                        return true;
                    }
                }
            }
            // bits(N) ≡ vector(N, bit) (Sail equivalence). Structural,
            // both directions handled.
            if expected_name == "bits" {
                if let TyKind::App { name: a_name, args: a_args, .. } = actual.kind() {
                    if a_name == "vector" {
                        let actual_n = a_args.first().and_then(|a| a.as_value_str());
                        let elem = a_args.get(1).and_then(|a| {
                            if let TyArg::Type(t) = a { Some(t) } else { None }
                        });
                        if let (Some(actual_n), Some(elem)) = (actual_n, elem) {
                            if matches!(elem.kind(), TyKind::Scalar(crate::ty::Scalar::Bit)) {
                                if let Some(expected_n) = expected_args.first().and_then(|a| a.as_value_str()) {
                                    return unify_value(&expected_n, &actual_n, subst);
                                }
                            }
                        }
                    }
                }
            }
            if expected_name == "vector" {
                if let Some(actual_n) = bits_width(actual) {
                    if let Some(expected_n) = expected_args.first().and_then(|a| a.as_value_str()) {
                        if let Some(TyArg::Type(elem)) = expected_args.get(1) {
                            if matches!(elem.kind(), TyKind::Scalar(crate::ty::Scalar::Bit)) {
                                return unify_value(&expected_n, &actual_n, subst);
                            }
                        }
                    }
                }
            }
            match actual.kind() {
                TyKind::App { name: actual_name, args: actual_args, .. }
                    if expected_name == actual_name && expected_args.len() == actual_args.len() =>
                {
                    expected_args.iter().zip(actual_args.iter()).all(|(expected, actual)| {
                        match (expected, actual) {
                            (TyArg::Type(expected), TyArg::Type(actual)) => {
                                unify_inner(expected, actual, subst, depth + 1)
                            }
                            // Both numeric (Value or Nexp) — compare as strings
                            (exp, act)
                                if exp.as_value_str().is_some() && act.as_value_str().is_some() =>
                            {
                                let e = exp.as_value_str().unwrap();
                                let a = act.as_value_str().unwrap();
                                unify_value(&e, &a, subst)
                            }
                            // Numeric expected vs type actual
                            (exp, TyArg::Type(actual)) if exp.as_value_str().is_some() => {
                                let e = exp.as_value_str().unwrap();
                                unify_value(&e, &actual.display_text(), subst)
                            }
                            // Type expected vs numeric actual
                            (TyArg::Type(expected), act) if act.as_value_str().is_some() => {
                                let a = act.as_value_str().unwrap();
                                unify_inner(expected, &Ty::named(a), subst, depth + 1)
                            }
                            _ => false,
                        }
                    })
                }
                _ => false,
            }
        }
        // + Existential in expected position — check with witness extraction.
        //
        // `exist 'n, constraint. T` in expected position means `actual` must
        // satisfy the inner type `T` for some valid witness values of the
        // quantified variables that satisfy `constraint`.
        TyKind::Exist { vars, constraint, inner } => {
            let ok = unify_inner(inner, actual, subst, depth + 1);
            if ok && !vars.is_empty() {
                use super::existential;
                let mut table = super::InferenceTable::default();
                match existential::extract_witnesses(vars, constraint, inner, actual, &mut table) {
                    existential::WitnessResult::ConstraintViolation { .. } => {
                        return false;
                    }
                    _ => {}
                }
            }
            ok
        }
        // Bidirectional type — unify structurally.
        TyKind::Bidir { lhs, rhs } => match actual.kind() {
            TyKind::Bidir { lhs: a_lhs, rhs: a_rhs } => {
                unify_inner(lhs, a_lhs, subst, depth + 1)
                    && unify_inner(rhs, a_rhs, subst, depth + 1)
            }
            _ => false,
        },
        TyKind::Abstract { name, .. } => {
            // Abstract types unify only with the same abstract type name
            matches!(actual.kind(), TyKind::Abstract { name: n, .. } if n == name)
        }
    }
}

/// Pretty-print a `TypeScheme` as a Sail-style signature string.
pub(super) fn format_scheme_signature(scheme: &TypeScheme) -> String {
    let mut header = String::new();
    if !scheme.quantifiers.is_empty() || !scheme.constraints.is_empty() {
        header.push_str("forall ");
        let parts = scheme
            .quantifiers
            .iter()
            .cloned()
            .chain(scheme.constraints.iter().map(|c| c.text.clone()))
            .collect::<Vec<_>>();
        header.push_str(&parts.join(", "));
        header.push_str(". ");
    }
    let params = scheme.params.iter().map(|p| p.display_text()).collect::<Vec<_>>().join(", ");
    let ret = scheme.ret.display_text();
    if scheme.params.len() == 1 {
        format!("{header}{params} -> {ret}")
    } else {
        format!("{header}({params}) -> {ret}")
    }
}

// try fast path → try assumption-based → return Deferred for batch solving.

/// Result of attempting to normalize a constraint.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // infrastructure — wired in A-3.4
pub(super) enum NormalizationResult {
    Decided(ConstraintStatus),
    Deferred,
}

/// Algebraic simplification of numeric expressions.
pub(super) fn simplify_numeric_expr(expr: &NumericExpr) -> NumericExpr {
    use NumericExpr::*;
    match expr {
        // x + 0 → x, 0 + x → x
        Add(a, b) => {
            let a = simplify_numeric_expr(a);
            let b = simplify_numeric_expr(b);
            match (&a, &b) {
                (Const(0), _) => b,
                (_, Const(0)) => a,
                (Const(x), Const(y)) => Const(x + y),
                _ => Add(Box::new(a), Box::new(b)),
            }
        }
        // x - 0 → x, x - x → 0
        Sub(a, b) => {
            let a = simplify_numeric_expr(a);
            let b = simplify_numeric_expr(b);
            match (&a, &b) {
                (_, Const(0)) => a,
                (Const(x), Const(y)) => Const(x - y),
                _ if a == b => Const(0),
                _ => Sub(Box::new(a), Box::new(b)),
            }
        }
        // x * 1 → x, 1 * x → x, x * 0 → 0
        Mul(a, b) => {
            let a = simplify_numeric_expr(a);
            let b = simplify_numeric_expr(b);
            match (&a, &b) {
                (Const(0), _) | (_, Const(0)) => Const(0),
                (Const(1), _) => b,
                (_, Const(1)) => a,
                (Const(x), Const(y)) => Const(x * y),
                _ => Mul(Box::new(a), Box::new(b)),
            }
        }
        // 2^const folding
        Exp(inner) => {
            let inner = simplify_numeric_expr(inner);
            if let Const(n) = &inner {
                if *n >= 0 && *n <= 63 {
                    return Const(1i64 << n);
                }
            }
            Exp(Box::new(inner))
        }
        // -0 → 0, -(const) → const
        Neg(inner) => {
            let inner = simplify_numeric_expr(inner);
            match &inner {
                Const(0) => Const(0),
                Const(n) => Const(-n),
                _ => Neg(Box::new(inner)),
            }
        }
        // Leaves
        other => other.clone(),
    }
}

/// Simplify a constraint expression by simplifying its numeric sub-expressions.
fn simplify_constraint_expr(expr: &ConstraintExpr) -> ConstraintExpr {
    match expr {
        ConstraintExpr::Compare { lhs, op, rhs } => ConstraintExpr::Compare {
            lhs: simplify_numeric_expr(lhs),
            op: *op,
            rhs: simplify_numeric_expr(rhs),
        },
        ConstraintExpr::And(items) => {
            ConstraintExpr::And(items.iter().map(simplify_constraint_expr).collect())
        }
        ConstraintExpr::Or(items) => {
            ConstraintExpr::Or(items.iter().map(simplify_constraint_expr).collect())
        }
        ConstraintExpr::Not(inner) => {
            ConstraintExpr::Not(Box::new(simplify_constraint_expr(inner)))
        }
        other => other.clone(),
    }
}

/// Unified constraint normalization entry point.
#[allow(dead_code)] // infrastructure — wired in A-3.4
pub(super) fn try_normalize_constraint(
    constraint: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> NormalizationResult {
    // Simplify constraint before normalization
    let constraint = &simplify_constraint_expr(constraint);

    // Trivially true/false constraints
    match constraint {
        ConstraintExpr::Bool(true) => {
            return NormalizationResult::Decided(ConstraintStatus::Satisfied)
        }
        ConstraintExpr::Bool(false) => {
            return NormalizationResult::Decided(ConstraintStatus::Failed)
        }
        ConstraintExpr::Unsupported => return NormalizationResult::Deferred,
        _ => {}
    }

    // Fast path: direct assumption matching
    if assumptions.iter().any(|a| direct_constraint_match(a, constraint, subst)) {
        return NormalizationResult::Decided(ConstraintStatus::Satisfied);
    }

    // Medium path: fact-based implication via polynomial solver
    if constraint_implied_by_assumptions(assumptions, constraint, subst) {
        return NormalizationResult::Decided(ConstraintStatus::Satisfied);
    }

    // Check contradiction
    if let ConstraintExpr::Not(inner) = constraint {
        if constraint_implied_by_assumptions(assumptions, inner, subst) {
            return NormalizationResult::Decided(ConstraintStatus::Failed);
        }
    }

    // Z3 fallback for constraints that polynomial/fact-based
    // strategies can't resolve.
    #[cfg(feature = "z3-solver")]
    {
        let status = z3_solver::try_solve(constraint, subst, assumptions);
        match status {
            ConstraintStatus::Satisfied => {
                return NormalizationResult::Decided(ConstraintStatus::Satisfied)
            }
            ConstraintStatus::Failed => {
                return NormalizationResult::Decided(ConstraintStatus::Failed)
            }
            ConstraintStatus::Unknown => {} // fall through to Deferred
        }
    }

    // Deferred: cannot resolve with current strategies
    NormalizationResult::Deferred
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polynomial_constant_folding_div() {
        // 10 / 2 = 5
        let expr =
            NumericExpr::Div(Box::new(NumericExpr::Const(10)), Box::new(NumericExpr::Const(2)));
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_some());
        assert_eq!(poly.unwrap().as_constant(), Some(5));
    }

    #[test]
    fn polynomial_constant_folding_mod() {
        // 10 % 3 = 1
        let expr =
            NumericExpr::Mod(Box::new(NumericExpr::Const(10)), Box::new(NumericExpr::Const(3)));
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_some());
        assert_eq!(poly.unwrap().as_constant(), Some(1));
    }

    #[test]
    fn polynomial_exp_constant() {
        // 2^8 = 256
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Const(8)));
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_some());
        assert_eq!(poly.unwrap().as_constant(), Some(256));
    }

    #[test]
    fn polynomial_exp_variable_returns_none() {
        // 2^n where n is a variable → can't be polynomial
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Var("n".to_string())));
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_none());
    }

    #[test]
    fn polynomial_div_by_zero_returns_none() {
        let expr =
            NumericExpr::Div(Box::new(NumericExpr::Const(10)), Box::new(NumericExpr::Const(0)));
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_none());
    }

    #[test]
    fn polynomial_div_with_variable_returns_none() {
        // 10 / n → not constant-foldable
        let expr = NumericExpr::Div(
            Box::new(NumericExpr::Const(10)),
            Box::new(NumericExpr::Var("n".to_string())),
        );
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_none());
    }

    #[test]
    fn polynomial_complex_exp_in_expression() {
        // 2^4 + 1 = 17
        let expr = NumericExpr::Add(
            Box::new(NumericExpr::Exp(Box::new(NumericExpr::Const(4)))),
            Box::new(NumericExpr::Const(1)),
        );
        let poly = polynomial_from_numeric_expr(&expr);
        assert!(poly.is_some());
        assert_eq!(poly.unwrap().as_constant(), Some(17));
    }

    #[test]
    fn contains_non_polynomial_detects_div() {
        use super::super::numeric::contains_non_polynomial;
        let expr = NumericExpr::Div(
            Box::new(NumericExpr::Var("n".to_string())),
            Box::new(NumericExpr::Const(2)),
        );
        assert!(contains_non_polynomial(&expr));
    }

    #[test]
    fn contains_non_polynomial_ok_for_add() {
        use super::super::numeric::contains_non_polynomial;
        let expr = NumericExpr::Add(
            Box::new(NumericExpr::Var("n".to_string())),
            Box::new(NumericExpr::Const(1)),
        );
        assert!(!contains_non_polynomial(&expr));
    }

    #[test]
    fn poly_div_by_constant_exact() {
        // 4n / 2 = 2n
        let four_n = Polynomial::variable("n".to_string()).mul(&Polynomial::constant(4)).unwrap();
        let result = four_n.div_by_constant(2).expect("4n / 2 should succeed");
        let expected = Polynomial::variable("n".to_string()).mul(&Polynomial::constant(2)).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn poly_div_by_constant_not_exact() {
        // 3n / 2 → None (not evenly divisible)
        let three_n = Polynomial::variable("n".to_string()).mul(&Polynomial::constant(3)).unwrap();
        assert!(three_n.div_by_constant(2).is_none());
    }

    #[test]
    fn poly_div_constant_by_constant() {
        // 6 / 3 = 2
        let expr =
            NumericExpr::Div(Box::new(NumericExpr::Const(6)), Box::new(NumericExpr::Const(3)));
        let poly = polynomial_from_numeric_expr(&expr).unwrap();
        assert_eq!(poly.as_constant(), Some(2));
    }

    #[test]
    fn poly_div_symbolic_by_constant_exact() {
        // (4*n) / 2 → 2*n
        let expr = NumericExpr::Div(
            Box::new(NumericExpr::Mul(
                Box::new(NumericExpr::Const(4)),
                Box::new(NumericExpr::Var("n".to_string())),
            )),
            Box::new(NumericExpr::Const(2)),
        );
        let poly = polynomial_from_numeric_expr(&expr).unwrap();
        let expected = Polynomial::variable("n".to_string()).mul(&Polynomial::constant(2)).unwrap();
        assert_eq!(poly, expected);
    }

    #[test]
    fn poly_mod_constants() {
        // 7 % 3 = 1
        let expr =
            NumericExpr::Mod(Box::new(NumericExpr::Const(7)), Box::new(NumericExpr::Const(3)));
        let poly = polynomial_from_numeric_expr(&expr).unwrap();
        assert_eq!(poly.as_constant(), Some(1));
    }

    #[test]
    fn poly_exp_constant() {
        // 2^5 = 32
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Const(5)));
        let poly = polynomial_from_numeric_expr(&expr).unwrap();
        assert_eq!(poly.as_constant(), Some(32));
    }

    #[test]
    fn poly_exp_symbolic_returns_none() {
        // 2^n → None (not polynomial)
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Var("n".to_string())));
        assert!(polynomial_from_numeric_expr(&expr).is_none());
    }

    #[test]
    fn poly_div_by_zero_returns_none() {
        let expr =
            NumericExpr::Div(Box::new(NumericExpr::Const(10)), Box::new(NumericExpr::Const(0)));
        assert!(polynomial_from_numeric_expr(&expr).is_none());
    }

    /// bit and bitvector(1) are unified.
    #[test]
    fn bits_width_returns_1_for_bit() {
        let bit_ty = Ty::named("bit");
        assert_eq!(bits_width(&bit_ty), Some("1".to_string()));
    }

    #[test]
    fn bits_width_returns_width_for_bits_n() {
        let bits8 = Ty::app("bits", vec![TyArg::numeric("8")], "bits(8)");
        assert_eq!(bits_width(&bits8), Some("8".to_string()));

        let bits1 = Ty::app("bits", vec![TyArg::numeric("1")], "bits(1)");
        assert_eq!(bits_width(&bits1), Some("1".to_string()));
    }

    /// bit should unify with bits(1) in both directions.
    #[test]
    fn bit_unifies_with_bits_1() {
        let bit_ty = Ty::named("bit");
        let bits1 = Ty::app("bits", vec![TyArg::numeric("1")], "bits(1)");

        let mut subst1 = Subst::default();
        let fwd = unify(&bit_ty, &bits1, &mut subst1);

        let mut subst2 = Subst::default();
        let rev = unify(&bits1, &bit_ty, &mut subst2);

        assert!(
            fwd || rev,
            "bit should unify with bits(1) in at least one direction (fwd={fwd}, rev={rev})"
        );
        assert!(fwd, "expected=bit, actual=bits(1) should unify");
        assert!(rev, "expected=bits(1), actual=bit should unify");
    }

    /// bit should NOT unify with bits(2).
    #[test]
    fn bit_does_not_unify_with_bits_2() {
        let bit_ty = Ty::named("bit");
        let bits2 = Ty::app("bits", vec![TyArg::numeric("2")], "bits(2)");
        let mut subst = Subst::default();

        assert!(!unify(&bit_ty, &bits2, &mut subst), "bit should not unify with bits(2)");
    }

    /// Parse `if 'n == 32 then 22 else 44` as NumericExpr::If.
    #[test]
    fn parse_if_then_else_numeric() {
        use super::numeric::parse_numeric_expr_text;
        let expr = parse_numeric_expr_text("if 'n == 32 then 22 else 44");
        assert!(expr.is_some(), "should parse if-then-else");
        match expr.unwrap() {
            NumericExpr::If { cond, then_expr, else_expr } => {
                let cond_text = cond.to_text();
                assert!(cond_text.contains("=="), "cond should contain ==: {cond_text}");
                assert!(matches!(*then_expr, NumericExpr::Const(22)));
                assert!(matches!(*else_expr, NumericExpr::Const(44)));
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    /// Same if-then-else expressions should unify.
    #[test]
    fn if_then_else_same_unifies() {
        let a = Ty::app(
            "bits",
            vec![TyArg::numeric("if 'n == 32 then 22 else 44")],
            "bits(if 'n == 32 then 22 else 44)",
        );
        let b = Ty::app(
            "bits",
            vec![TyArg::numeric("if 'n == 32 then 22 else 44")],
            "bits(if 'n == 32 then 22 else 44)",
        );
        let mut subst = Subst::default();
        assert!(unify(&a, &b, &mut subst), "same if-then-else should unify");
    }

    /// Different if-then-else branches should still unify (permissive
    /// for now, since LSP can't evaluate conditions without full context).
    #[test]
    fn if_then_else_different_cond_permissive() {
        let a = Ty::app("bits", vec![TyArg::numeric("if 'n == 32 then 22 else 44")], "");
        let b = Ty::app("bits", vec![TyArg::numeric("64")], "bits(64)");
        let mut subst = Subst::default();
        // Permissive: If on one side, non-If on other → accept
        assert!(unify(&a, &b, &mut subst), "if-then-else vs plain should be permissive");
    }
}
