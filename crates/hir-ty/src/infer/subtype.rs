//! Directional subtype checks. Unknown constraints are retained, not proved.
use super::*;

#[derive(Clone, Debug)]
pub(super) enum SubtypeResult {
    Ok,
    NeedsConstraint(Obligation),
    Fail,
}

fn goal(expr: ConstraintExpr) -> SubtypeResult {
    if let ConstraintExpr::Compare { lhs, op, rhs } = &expr {
        if *op == CompareOp::Eq
            && (matches!(lhs, NumericExpr::If { .. }) || matches!(rhs, NumericExpr::If { .. }))
        {
            return match super::numeric::numeric_equality_without_binding(
                lhs,
                rhs,
                &Subst::default(),
                &[],
            ) {
                ConstraintStatus::Satisfied => SubtypeResult::Ok,
                ConstraintStatus::Failed => SubtypeResult::Fail,
                ConstraintStatus::Unknown => {
                    SubtypeResult::NeedsConstraint(Obligation::NumericConstraint(expr))
                }
            };
        }
        let decided = match op {
            CompareOp::Eq => super::nexp_simplify::check_eq(lhs, rhs),
            CompareOp::Lte => super::nexp_simplify::check_le(lhs, rhs),
            _ => None,
        };
        if let Some(holds) = decided {
            return if holds { SubtypeResult::Ok } else { SubtypeResult::Fail };
        }
    }
    match super::solver::solve_without_smt(&expr, &Subst::default(), &[]) {
        ConstraintStatus::Satisfied => SubtypeResult::Ok,
        ConstraintStatus::Failed => SubtypeResult::Fail,
        ConstraintStatus::Unknown => {
            SubtypeResult::NeedsConstraint(Obligation::NumericConstraint(expr))
        }
    }
}
fn undecided() -> SubtypeResult {
    goal(ConstraintExpr::Unsupported)
}
fn comparison(lhs: NumericExpr, op: CompareOp, rhs: NumericExpr) -> ConstraintExpr {
    ConstraintExpr::Compare { lhs, op, rhs }
}
/// Retain local assumptions on quantified components rather than flattening
/// them into facts shared by unrelated tuple/function components.
fn combine(results: impl IntoIterator<Item = SubtypeResult>) -> SubtypeResult {
    let mut pending = Vec::new();
    for result in results {
        match result {
            SubtypeResult::Fail => return SubtypeResult::Fail,
            SubtypeResult::NeedsConstraint(ob) => pending.push(ob),
            SubtypeResult::Ok => {}
        }
    }
    if pending.is_empty() {
        SubtypeResult::Ok
    } else if pending
        .iter()
        .any(|ob| matches!(ob, Obligation::Assuming { .. } | Obligation::All(_)))
    {
        SubtypeResult::NeedsConstraint(Obligation::All(pending))
    } else {
        SubtypeResult::NeedsConstraint(Obligation::NumericConstraint(ConstraintExpr::And(
            pending.iter().map(Obligation::constraint).collect(),
        )))
    }
}
fn nexp(arg: &TyArg) -> Option<NumericExpr> {
    let expr = match arg {
        TyArg::Nexp(n) => Some(n.clone()),
        TyArg::Value(v) => NumericExpr::parse(v),
        TyArg::Type(t) if !matches!(t.kind(), TyKind::Infer(_) | TyKind::Error) => {
            NumericExpr::parse(&t.display_text())
        }
        _ => None,
    }?;
    if contains_erased_numeric(&expr) {
        None
    } else {
        Some(expr)
    }
}
pub(super) fn contains_erased_numeric(expr: &NumericExpr) -> bool {
    match expr {
        NumericExpr::Symbol(name) | NumericExpr::Var(name) => {
            matches!(name.as_str(), crate::ty::UNRESOLVED_VAR | "int" | "nat" | "bool")
        }
        NumericExpr::Const(_) | NumericExpr::BigConst(_) => false,
        NumericExpr::Neg(a) | NumericExpr::Exp(a) => contains_erased_numeric(a),
        NumericExpr::Add(a, b)
        | NumericExpr::Sub(a, b)
        | NumericExpr::Mul(a, b)
        | NumericExpr::Div(a, b)
        | NumericExpr::Mod(a, b) => contains_erased_numeric(a) || contains_erased_numeric(b),
        NumericExpr::App { args, .. } => args.iter().any(contains_erased_numeric),
        NumericExpr::If { then_expr, else_expr, .. } => {
            contains_erased_numeric(then_expr) || contains_erased_numeric(else_expr)
        }
    }
}
/// Erased bounds are distinct from mathematical infinity.
#[derive(Clone)]
enum Bound {
    Infinite,
    Unknown,
    Known(NumericExpr),
}
fn bound(arg: &TyArg) -> Bound {
    nexp(arg).map_or(Bound::Unknown, Bound::Known)
}
fn interval(ty: &Ty) -> Option<(Bound, Bound)> {
    use Bound::*;
    match ty.kind() {
        TyKind::Scalar(crate::ty::Scalar::Int) => Some((Infinite, Infinite)),
        TyKind::Scalar(crate::ty::Scalar::Nat) => Some((Known(NumericExpr::Const(0)), Infinite)),
        TyKind::App { name, args, .. } => match (name.as_str(), args.as_slice()) {
            ("atom" | "int" | "nat" | "implicit", [n]) => Some((bound(n), bound(n))),
            ("range", [lo, hi]) => Some((bound(lo), bound(hi))),
            _ => None,
        },
        _ => None,
    }
}
fn contains_bound(actual: Bound, expected: Bound, lower: bool) -> SubtypeResult {
    use Bound::*;
    match (actual, expected) {
        (_, Infinite) => SubtypeResult::Ok,
        (_, Unknown) | (Unknown, _) => undecided(),
        (Infinite, Known(_)) => SubtypeResult::Fail,
        (Known(a), Known(e)) => goal(if lower {
            comparison(e, CompareOp::Lte, a)
        } else {
            comparison(a, CompareOp::Lte, e)
        }),
    }
}
pub(super) fn bool_value(ty: &Ty) -> Option<ConstraintExpr> {
    let TyKind::App { name, args, .. } = ty.kind() else {
        return None;
    };
    if !matches!(name.as_str(), "bool" | "atom_bool") || args.len() != 1 {
        return None;
    }
    let text = match &args[0] {
        TyArg::Bool(c) => return Some(c.clone()),
        TyArg::Type(t) => t.display_text(),
        arg => arg.as_value_str()?,
    };
    Some(parse_constraint_text(&text))
}

fn canonical_application(ty: Ty) -> Ty {
    let ty = match ty.kind() {
        TyKind::Named(name, args) if !args.is_empty() => {
            Ty::app(name, args.clone(), ty.display_text())
        }
        _ => ty,
    };
    match ty.kind() {
        TyKind::Scalar(crate::ty::Scalar::Bit) => {
            Ty::app("bits", vec![TyArg::numeric("1")], "bits(1)")
        }
        TyKind::App { name, args, .. } if name == "implicit" => {
            Ty::app("atom", args.clone(), ty.display_text())
        }
        TyKind::App { name, args, .. } if name == "bitvector" && !args.is_empty() => {
            Ty::app("bits", vec![args[0].clone()], ty.display_text())
        }
        _ => ty,
    }
}

/// Check `sub <: sup`. Failed checks restore speculative bindings.
pub(super) fn is_subtype(table: &mut InferenceTable, sub: &Ty, sup: &Ty) -> SubtypeResult {
    let snapshot = table.snapshot();
    let result = check(table, sub, sup, 0);
    if matches!(result, SubtypeResult::Fail) {
        table.rollback_to(snapshot);
    } else {
        table.commit(snapshot);
    }
    result
}
fn check(table: &mut InferenceTable, sub: &Ty, sup: &Ty, depth: usize) -> SubtypeResult {
    if depth > 96 {
        return undecided();
    }
    let sub = table.shallow_resolve(sub);
    let sup = table.shallow_resolve(sup);
    if sub.is_error() || sup.is_error() {
        return undecided();
    }
    if sub == sup {
        return SubtypeResult::Ok;
    }
    // Preserve source-level aliases when binding an inference variable.
    match (sub.kind(), sup.kind()) {
        (TyKind::Infer(crate::ty::InferTy(id)), _) => {
            return if table.bind(*id, sup) { SubtypeResult::Ok } else { SubtypeResult::Fail }
        }
        (_, TyKind::Infer(crate::ty::InferTy(id))) => {
            return if table.bind(*id, sub) { SubtypeResult::Ok } else { SubtypeResult::Fail }
        }
        _ => {}
    }
    let sub = canonical_application(table.normalize_alias_deep(&sub));
    let sup = canonical_application(table.normalize_alias_deep(&sup));
    // Preserve a vector's element argument when comparing two generic
    // vectors. Erasing it to bits before inference loses the element binding.
    let as_bits = |ty: Ty, other: &Ty| {
        if other.ctor_name() == Some("bits") {
            if let TyKind::App { name, args, .. } = ty.kind() {
                if name == "vector"
                    && args.len() >= 2
                    && matches!(args.last(), Some(TyArg::Type(t)) if t.is_named("bit"))
                {
                    return Ty::app("bits", vec![args[0].clone()], ty.display_text());
                }
            }
        }
        ty
    };
    let sub = as_bits(sub, &sup);
    let sup = as_bits(sup, &sub);
    if sub.is_error() || sup.is_error() {
        return undecided();
    }
    if sub == sup {
        return SubtypeResult::Ok;
    }
    if let TyKind::Exist { vars, constraint, inner } = sub.kind() {
        let universe = table.universe();
        let opened = table.open_existential_avoiding(vars, constraint, inner, &sup);
        let result = check(table, &opened.inner_ty, &sup, depth + 1);
        table.restore_universe(universe);
        return match result {
            SubtypeResult::NeedsConstraint(goal) => {
                let goal_expr = table.resolve_constraint(&goal.constraint());
                match super::solver::solve_without_smt(
                    &goal_expr,
                    &Subst::default(),
                    &[opened.constraint.clone()],
                ) {
                    ConstraintStatus::Satisfied => SubtypeResult::Ok,
                    ConstraintStatus::Failed => SubtypeResult::Fail,
                    ConstraintStatus::Unknown => {
                        SubtypeResult::NeedsConstraint(Obligation::Assuming {
                            assumptions: vec![opened.constraint],
                            goal: Box::new(goal),
                        })
                    }
                }
            }
            result => result,
        };
    }
    if let TyKind::Exist { vars, constraint, inner } = sup.kind() {
        if existential_admits(table, &sup, &sub) {
            return SubtypeResult::Ok;
        }
        let (inner, predicate) =
            table.instantiate_existential_avoiding(vars, constraint, inner, &sub);
        let _ = table.infer_bindings(&inner, &sub);
        let inner = table.deep_resolve(&inner);
        let result = check(table, &sub, &inner, depth + 1);
        let predicate = table.resolve_constraint(&predicate);
        return combine([result, goal(predicate)]);
    }
    if matches!(sub.kind(), TyKind::Param(n) if table.is_rigid(n))
        || matches!(sup.kind(), TyKind::Param(n) if table.is_rigid(n))
    {
        return SubtypeResult::Fail;
    }
    // Widening to int needs no evidence about an unresolved singleton bound.
    if matches!(sup.kind(), TyKind::Scalar(crate::ty::Scalar::Int))
        && matches!(sub.kind(), TyKind::App {name,..} if matches!(name.as_str(), "int"|"atom"|"nat"|"range"|"implicit"))
    {
        return SubtypeResult::Ok;
    }
    match (sub.kind(), sup.kind()) {
        (TyKind::Tuple(items), _) if items.len() == 1 => {
            return check(table, &items[0], &sup, depth + 1)
        }
        (_, TyKind::Tuple(items)) if items.len() == 1 => {
            return check(table, &sub, &items[0], depth + 1)
        }
        _ => {}
    }
    if let (Some((lo, hi)), Some((expected_lo, expected_hi))) = (interval(&sub), interval(&sup)) {
        if let (Bound::Known(lo), Bound::Known(hi), Bound::Known(elo), Bound::Known(ehi)) =
            (&lo, &hi, &expected_lo, &expected_hi)
        {
            if lo == hi && elo == ehi {
                let result = super::numeric::numeric_equality_without_binding(
                    lo,
                    elo,
                    &Subst::default(),
                    &[],
                );
                return match result {
                    ConstraintStatus::Satisfied => SubtypeResult::Ok,
                    ConstraintStatus::Failed => SubtypeResult::Fail,
                    ConstraintStatus::Unknown => {
                        goal(comparison(lo.clone(), CompareOp::Eq, elo.clone()))
                    }
                };
            }
        }
        if let (Bound::Known(lo), Bound::Known(hi)) = (&lo, &hi) {
            if matches!(
                goal(comparison(lo.clone(), CompareOp::Lte, hi.clone())),
                SubtypeResult::Fail
            ) {
                return SubtypeResult::Ok;
            }
        }
        return combine([
            contains_bound(lo, expected_lo, true),
            contains_bound(hi, expected_hi, false),
        ]);
    }
    if let Some(actual) = bool_value(&sub) {
        if matches!(sup.kind(), TyKind::Scalar(crate::ty::Scalar::Bool)) {
            return SubtypeResult::Ok;
        }
        if let Some(expected) = bool_value(&sup) {
            let actual = table.resolve_constraint(&actual);
            let expected = table.resolve_constraint(&expected);
            table.bind_boolean_index(&expected, &actual);
            table.bind_boolean_index(&actual, &expected);
            let actual = table.resolve_constraint(&actual);
            let expected = table.resolve_constraint(&expected);
            if actual == expected {
                return SubtypeResult::Ok;
            }
            return goal(ConstraintExpr::And(vec![
                ConstraintExpr::Or(vec![
                    ConstraintExpr::Not(Box::new(actual.clone())),
                    expected.clone(),
                ]),
                ConstraintExpr::Or(vec![ConstraintExpr::Not(Box::new(expected)), actual]),
            ]));
        }
    }
    if matches!(sub.kind(), TyKind::Scalar(crate::ty::Scalar::Bool)) && bool_value(&sup).is_some() {
        return SubtypeResult::Fail;
    }
    match (sub.kind(), sup.kind()) {
        (TyKind::Tuple(xs), TyKind::Tuple(ys)) => {
            if xs.len() != ys.len() {
                return SubtypeResult::Fail;
            }
            combine(xs.iter().zip(ys).map(|(x, y)| check(table, x, y, depth + 1)))
        }
        (TyKind::FnPtr(x), TyKind::FnPtr(y)) => {
            if x.params.len() != y.params.len() {
                return SubtypeResult::Fail;
            }
            let mut results: Vec<_> = y
                .params
                .iter()
                .zip(&x.params)
                .map(|(p, a)| check(table, p, a, depth + 1))
                .collect();
            results.push(check(table, &x.ret, &y.ret, depth + 1));
            combine(results)
        }
        (TyKind::App { name: a, args: xs, .. }, TyKind::App { name: b, args: ys, .. })
            if a == b =>
        {
            if xs.len() != ys.len() {
                return SubtypeResult::Fail;
            }
            combine(xs.iter().zip(ys).enumerate().map(|(i, (x, y))| {
                if x == y {
                    return SubtypeResult::Ok;
                }
                let numeric = super::kinds::arguments(a, xs.len()).and_then(|ks| ks.get(i))
                    == Some(&crate::ty::Kind::Int);
                let (x, y) = if numeric {
                    (super::kinds::numeric_argument(x), super::kinds::numeric_argument(y))
                } else {
                    (std::borrow::Cow::Borrowed(x), std::borrow::Cow::Borrowed(y))
                };
                if numeric {
                    for (variable, value) in [(y.as_ref(), x.as_ref()), (x.as_ref(), y.as_ref())] {
                        if let TyArg::Type(variable) = variable {
                            if let TyKind::Infer(id) = variable.kind() {
                                if let Some(value) = nexp(value) {
                                    return if table.bind(id.0, Ty::param(value.to_string_repr())) {
                                        SubtypeResult::Ok
                                    } else {
                                        SubtypeResult::Fail
                                    };
                                }
                            }
                        }
                    }
                }
                match (x.as_ref(), y.as_ref()) {
                    _ if x == y => SubtypeResult::Ok,
                    (TyArg::Bool(x), TyArg::Bool(y)) => goal(ConstraintExpr::And(vec![
                        ConstraintExpr::Or(vec![
                            ConstraintExpr::Not(Box::new(x.clone())),
                            y.clone(),
                        ]),
                        ConstraintExpr::Or(vec![
                            ConstraintExpr::Not(Box::new(y.clone())),
                            x.clone(),
                        ]),
                    ])),
                    (TyArg::Order(x), TyArg::Order(y))
                        if matches!(x.as_str(), "inc" | "dec")
                            && matches!(y.as_str(), "inc" | "dec") =>
                    {
                        SubtypeResult::Fail
                    }
                    (TyArg::Bool(_), TyArg::Nexp(_) | TyArg::Order(_))
                    | (TyArg::Nexp(_) | TyArg::Order(_), TyArg::Bool(_))
                    | (TyArg::Order(_), TyArg::Nexp(_))
                    | (TyArg::Nexp(_), TyArg::Order(_)) => SubtypeResult::Fail,
                    (TyArg::Type(x), TyArg::Type(y)) => check(table, x, y, depth + 1),
                    _ => match (nexp(&x), nexp(&y)) {
                        (Some(x), Some(y)) => goal(comparison(x, CompareOp::Eq, y)),
                        _ => undecided(),
                    },
                }
            }))
        }
        (TyKind::Abstract { name: a, kind: ka }, TyKind::Abstract { name: b, kind: kb }) => {
            if a == b && ka == kb {
                SubtypeResult::Ok
            } else {
                SubtypeResult::Fail
            }
        }
        (TyKind::Param(_), _) | (_, TyKind::Param(_)) => undecided(),
        // Alias/configuration recovery is not a proof of subtyping. Preserve
        // permissive editor recovery, but expose the obligation as undecided.
        (TyKind::Named(..), _)
        | (_, TyKind::Named(..))
        | (TyKind::Abstract { .. }, _)
        | (_, TyKind::Abstract { .. }) => {
            if table.recover_shape_bindings(&sup, &sub) {
                undecided()
            } else {
                SubtypeResult::Fail
            }
        }
        // Legacy structural compatibility also includes recovery heuristics.
        // It must not be promoted to a subtype proof.
        _ => {
            if table.recover_shape_bindings(&sup, &sub) {
                undecided()
            } else {
                SubtypeResult::Fail
            }
        }
    }
}

/// `range(0, 31)` is admitted by `{'n, 0 <= 'n < 32. atom('n)}`.
///
/// Existential *introduction*: pick the witness. `unify` handles an
/// existential by freshening its variables and unifying the body, which asks
/// the opposite question — whether one particular `atom(?k)` equals
/// `range(0, 31)`, which nothing can — so `function regbits_to_regno b =
/// unsigned(b)` against `-> {'n, 0 <= 'n < 32. regno('n)}` was rejected in
/// five of upstream's `smt` files.
///
/// Only an interval. `'n in {8, 16, 32}` is not one, and answering from its
/// extremes would accept the values between them.
pub(super) fn existential_admits(table: &mut InferenceTable, sup: &Ty, sub: &Ty) -> bool {
    let TyKind::Exist { vars, constraint, inner } = sup.kind() else { return false };
    if vars.len() != 1 {
        return false;
    }
    // `normalize_alias_deep`, because this only *classifies* the body:
    // `regno('n)` is a parameterised synonym for `atom('n)` and the shallow
    // form does not follow schemes.
    let body = table.normalize_alias_deep(inner);
    let witness = match body.kind() {
        TyKind::App { name, args, .. }
            if matches!(name.as_str(), "atom" | "int" | "implicit") && args.len() == 1 =>
        {
            match &args[0] {
                TyArg::Type(t) => t.display_text(),
                TyArg::Nexp(n) => n.to_string_repr(),
                TyArg::Bool(c) => c.to_text(),
                TyArg::Order(v) | TyArg::Value(v) => v.clone(),
            }
        }
        _ => return false,
    };
    if witness != vars[0] {
        return false;
    }
    let sub = table.normalize_alias_deep(sub);
    let (Some((lo, hi)), Some((clo, chi))) = (
        super::expr::concrete_range_bounds(&sub),
        super::constraint::interval_for_var(constraint, &vars[0]),
    ) else {
        return false;
    };
    clo <= lo && hi <= chi
}
