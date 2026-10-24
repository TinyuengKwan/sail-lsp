//! Runtime evidence for omitted implicit arguments (Sail's rewrite_sizeof).
use super::*;

/// None preserves recovery/unsupported expressions instead of inventing an
/// editor error. A nominal type's indices are not runtime projections.
pub(super) fn recoverable(
    value: &NumericExpr,
    witnesses: &[Ty],
    assumptions: &[ConstraintExpr],
) -> Option<bool> {
    if subtype::contains_erased_numeric(value) {
        return None;
    }
    if integer::eval(value, &Subst::default(), assumptions).is_some()
        || numeric::eval_numeric_expr(value, &Subst::default(), assumptions).is_some()
    {
        return Some(true);
    }
    let mut equalities = Vec::new();
    for ty in witnesses {
        if let Some(actual) = refinement::singleton(ty) {
            if &actual == value {
                return Some(true);
            }
            let equal =
                ConstraintExpr::Compare { lhs: value.clone(), op: CompareOp::Eq, rhs: actual };
            if solver::solve_without_smt(&equal, &Subst::default(), assumptions)
                == ConstraintStatus::Satisfied
            {
                return Some(true);
            }
            equalities.push(equal);
        }
        if matches!(value, NumericExpr::Var(_)) {
            if let TyKind::App { name, args, .. } | TyKind::Named(name, args) = ty.kind() {
                if matches!(name.as_str(), "bitvector" | "bits" | "vector") {
                    let width = args.first().and_then(|a| match a {
                        TyArg::Nexp(n) => Some(n.clone()),
                        TyArg::Value(s) => NumericExpr::parse(s),
                        TyArg::Type(t) => NumericExpr::parse(&t.display_text()),
                        _ => None,
                    });
                    if width.as_ref() == Some(value) {
                        return Some(true);
                    }
                }
            }
        }
    }
    // Search all direct projections before spending the shared SMT budget.
    if equalities
        .iter()
        .any(|c| solver::solve(c, &Subst::default(), assumptions) == ConstraintStatus::Satisfied)
    {
        return Some(true);
    }
    match value {
        NumericExpr::Const(_) | NumericExpr::BigConst(_) => Some(true),
        NumericExpr::Var(name) => {
            let domain = constant_domain(name, assumptions);
            if domain == Some(true) {
                return domain;
            }
            #[cfg(feature = "z3-solver")]
            if z3_solver::try_solve_unique(
                &ConstraintExpr::Bool(true),
                &Subst::default(),
                assumptions,
                name,
            )
            .is_some()
            {
                return Some(true);
            }
            // Unsupported constraints and solver timeouts are not evidence
            // that no unique value exists.
            domain
        }
        NumericExpr::Neg(n) | NumericExpr::Exp(n) => recoverable(n, witnesses, assumptions),
        NumericExpr::Add(a, b)
        | NumericExpr::Sub(a, b)
        | NumericExpr::Mul(a, b)
        | NumericExpr::Div(a, b)
        | NumericExpr::Mod(a, b) => {
            Some(recoverable(a, witnesses, assumptions)? && recoverable(b, witnesses, assumptions)?)
        }
        NumericExpr::Symbol(_) | NumericExpr::App { .. } | NumericExpr::If { .. } => None,
    }
}

// Decide only equality classes bounded by integer constants. Correlated or
// nonlinear domains remain unknown when the general solver cannot decide.
fn constant_domain(name: &str, assumptions: &[ConstraintExpr]) -> Option<bool> {
    fn flatten<'a>(c: &'a ConstraintExpr, out: &mut Vec<&'a ConstraintExpr>) {
        match c {
            ConstraintExpr::And(cs) => cs.iter().for_each(|c| flatten(c, out)),
            _ => out.push(c),
        }
    }
    let mut facts = Vec::new();
    assumptions.iter().for_each(|c| flatten(c, &mut facts));
    let mut equal = HashSet::from([name.to_owned()]);
    loop {
        let before = equal.len();
        for c in &facts {
            if let ConstraintExpr::Compare {
                lhs: NumericExpr::Var(a),
                op: CompareOp::Eq,
                rhs: NumericExpr::Var(b),
            } = c
            {
                if equal.contains(a) || equal.contains(b) {
                    equal.insert(a.clone());
                    equal.insert(b.clone());
                }
            }
        }
        if before == equal.len() {
            break;
        }
    }
    let mut subst = Subst::default();
    for other in &equal {
        if other != name {
            subst.values.insert(other.clone(), name.to_owned());
        }
    }
    let mut bounds = Vec::new();
    for fact in facts {
        if matches!(fact, ConstraintExpr::Unsupported) {
            return None;
        }
        let mut names = HashSet::new();
        substitution::constraint_names(fact, &mut names);
        if !equal.iter().any(|n| names.contains(n.trim_start_matches('\''))) {
            continue;
        }
        let fact = constraint::apply_subst_constraint_expr(fact, &subst);
        match &fact {
            ConstraintExpr::Compare { lhs, op: CompareOp::Eq, rhs } if lhs == rhs => {}
            ConstraintExpr::Compare { lhs, op, rhs }
                if *op != CompareOp::Neq
                    && ((matches!(lhs, NumericExpr::Var(n) if n == name)
                        && matches!(rhs, NumericExpr::Const(_) | NumericExpr::BigConst(_)))
                        || (matches!(rhs, NumericExpr::Var(n) if n == name)
                            && matches!(
                                lhs,
                                NumericExpr::Const(_) | NumericExpr::BigConst(_)
                            ))) =>
            {
                bounds.push(fact)
            }
            _ => return None,
        }
    }
    // The interval helper is bounded; a larger integer is unknown here,
    // rather than evidence that the value is non-unique.
    if bounds.iter().any(|c| {
        matches!(c,
        ConstraintExpr::Compare { lhs, rhs, .. } if [lhs, rhs].iter().any(|n|
            matches!(n, NumericExpr::BigConst(v) if v.to_string().parse::<i128>().is_err())))
    }) {
        return None;
    }
    Some(
        constraint::interval_for_var(&ConstraintExpr::And(bounds), name)
            .is_some_and(|(lo, hi)| lo == hi),
    )
}
