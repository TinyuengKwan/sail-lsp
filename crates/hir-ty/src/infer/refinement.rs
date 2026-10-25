//! Facts extracted from HIR, using the identity of singleton types rather than
//! source names. Coarse or mutable values without such an identity stay unknown.
use super::*;
use hir_def::{Body, Expr, ExprId};

pub(super) fn number(
    body: &Body,
    id: ExprId,
    lookup: &impl Fn(&str) -> Option<Ty>,
) -> Option<NumericExpr> {
    use NumericExpr as N;
    match body.expr(id)? {
        Expr::Literal(parser::Literal::Number(s))
            if !s.starts_with("0x") && !s.starts_with("0b") =>
        {
            N::integer_literal(s)
        }
        Expr::TypeVar(v) => Some(N::Var(v.clone())),
        Expr::SizeOf { nexp } => N::parse(nexp),
        Expr::Ident(name) => singleton(&lookup(name)?),
        Expr::Cast { expr, .. } | Expr::Attribute { expr, .. } => number(body, *expr, lookup),
        Expr::If { cond, then_branch, else_branch: Some(else_branch) } => Some(N::If {
            cond: Box::new(guard(body, *cond, lookup)?),
            then_expr: Box::new(number(body, *then_branch, lookup)?),
            else_expr: Box::new(number(body, *else_branch, lookup)?),
        }),
        Expr::UnaryOp { op, expr } if op.as_str() == "-" => {
            Some(N::Neg(Box::new(number(body, *expr, lookup)?)))
        }
        Expr::BinaryOp { lhs, op, rhs } => {
            let a = Box::new(number(body, *lhs, lookup)?);
            let b = Box::new(number(body, *rhs, lookup)?);
            Some(match op.as_str() {
                "+" => N::Add(a, b),
                "-" => N::Sub(a, b),
                "*" => N::Mul(a, b),
                "/" => N::Div(a, b),
                "%" => N::Mod(a, b),
                "^" if *a == N::Const(2) => N::Exp(b),
                _ => return None,
            })
        }
        _ => None,
    }
}
/// Recover numeric identities from already inferred subexpressions, including
/// calls such as vector length inside an arithmetic slice endpoint.
pub(super) fn number_from_types(
    body: &Body,
    id: ExprId,
    lookup: &impl Fn(&str) -> Option<Ty>,
    expression_type: &impl Fn(ExprId) -> Option<Ty>,
) -> Option<NumericExpr> {
    use NumericExpr as N;
    if let Some(value) = number(body, id, lookup) {
        return Some(value);
    }
    if let Some(value) = expression_type(id).as_ref().and_then(singleton) {
        return Some(value);
    }
    let recur = |id| number_from_types(body, id, lookup, expression_type);
    match body.expr(id)? {
        Expr::Cast { expr, .. } | Expr::Attribute { expr, .. } => recur(*expr),
        Expr::UnaryOp { op, expr } if op.as_str() == "-" => Some(N::Neg(Box::new(recur(*expr)?))),
        Expr::BinaryOp { lhs, op, rhs } => {
            let a = Box::new(recur(*lhs)?);
            let b = Box::new(recur(*rhs)?);
            Some(match op.as_str() {
                "+" => N::Add(a, b),
                "-" => N::Sub(a, b),
                "*" => N::Mul(a, b),
                "/" => N::Div(a, b),
                "%" => N::Mod(a, b),
                "^" if *a == N::Const(2) => N::Exp(b),
                _ => return None,
            })
        }
        _ => None,
    }
}

pub(super) fn singleton(ty: &Ty) -> Option<NumericExpr> {
    let (TyKind::App { name, args, .. } | TyKind::Named(name, args)) = ty.kind() else {
        return None;
    };
    if !matches!(name.as_str(), "atom" | "int" | "implicit") || args.len() != 1 {
        return None;
    }
    match &args[0] {
        TyArg::Nexp(n) => Some(n.clone()),
        TyArg::Type(t) => NumericExpr::parse(&t.display_text()),
        TyArg::Value(v) => NumericExpr::parse(v),
        _ => None,
    }
}
pub(super) fn guard(
    body: &Body,
    id: ExprId,
    lookup: &impl Fn(&str) -> Option<Ty>,
) -> Option<ConstraintExpr> {
    use ConstraintExpr as C;
    match body.expr(id)? {
        Expr::Literal(parser::Literal::Bool(b)) => Some(C::Bool(*b)),
        Expr::Constraint(text) => {
            let c = parse_constraint_text(
                text.trim().strip_prefix("constraint").unwrap_or(text).trim(),
            );
            (!matches!(c, C::Unsupported)).then_some(c)
        }
        Expr::Ident(name) => {
            let ty = lookup(name)?;
            let (TyKind::App { name, args, .. } | TyKind::Named(name, args)) = ty.kind() else {
                return None;
            };
            if !matches!(name.as_str(), "atom_bool" | "bool") || args.len() != 1 {
                return None;
            }
            match &args[0] {
                TyArg::Bool(c) => Some(c.clone()),
                TyArg::Type(t) => Some(parse_constraint_text(&t.display_text())),
                _ => None,
            }
        }
        Expr::UnaryOp { op, expr } if matches!(op.as_str(), "~" | "!" | "not") => {
            Some(C::Not(Box::new(guard(body, *expr, lookup)?)))
        }
        Expr::Call { callee, args }
            if args.len() == 1 && matches!(body.expr(*callee),Some(Expr::Ident(n)) if n=="not") =>
        {
            Some(C::Not(Box::new(guard(body, args[0], lookup)?)))
        }
        Expr::Cast { expr, .. } | Expr::Attribute { expr, .. } => guard(body, *expr, lookup),
        Expr::BinaryOp { lhs, op, rhs } => {
            if matches!(op.as_str(), "&" | "&&" | "and" | "|" | "||" | "or") {
                let a = guard(body, *lhs, lookup);
                let b = guard(body, *rhs, lookup);
                return if matches!(op.as_str(), "&" | "&&" | "and") {
                    Some(C::And(vec![a.unwrap_or(C::Unsupported), b.unwrap_or(C::Unsupported)]))
                } else {
                    Some(C::Or(vec![a?, b?]))
                };
            }
            let op = match op.as_str() {
                "==" => CompareOp::Eq,
                "!=" => CompareOp::Neq,
                "<" => CompareOp::Lt,
                "<=" => CompareOp::Lte,
                ">" => CompareOp::Gt,
                ">=" => CompareOp::Gte,
                _ => return None,
            };
            Some(C::Compare {
                lhs: number(body, *lhs, lookup)?,
                op,
                rhs: number(body, *rhs, lookup)?,
            })
        }
        _ => None,
    }
}

/// A structural least common supertype where representable. Fresh existential
/// indices preserve alternatives for invariant constructors without selecting a
/// branch's width. Unsupported joins return None to the caller's recovery path.
pub(super) fn join(a: &Ty, b: &Ty) -> Option<Ty> {
    use num_bigint::BigInt;
    fn bounds(t: &Ty) -> Option<(BigInt, BigInt)> {
        if let Some(n) = singleton(t) {
            let v = super::integer::eval(&n, &Subst::default(), &[])?;
            return Some((v.clone(), v));
        }
        let (TyKind::App { name, args, .. } | TyKind::Named(name, args)) = t.kind() else {
            return None;
        };
        if name != "range" || args.len() != 2 {
            return None;
        }
        let eval = |a: &TyArg| {
            super::integer::eval(&NumericExpr::parse(&a.as_value_str()?)?, &Subst::default(), &[])
        };
        Some((eval(&args[0])?, eval(&args[1])?))
    }
    if a == b {
        return Some(a.clone());
    }
    fn boolean(t: &Ty) -> bool {
        if let TyKind::Exist { inner, .. } = t.kind() {
            return boolean(inner);
        }
        t.is_named("bool") || super::subtype::bool_value(t).is_some()
    }
    if boolean(a) && boolean(b) {
        return Some(Ty::named("bool"));
    }
    if let (Some((lo, hi)), Some((lo2, hi2))) = (bounds(a), bounds(b)) {
        return Some(Ty::app(
            "range",
            vec![
                TyArg::Nexp(NumericExpr::from_integer(lo.min(lo2))),
                TyArg::Nexp(NumericExpr::from_integer(hi.max(hi2))),
            ],
            "",
        ));
    }
    if let (TyKind::Tuple(xs), TyKind::Tuple(ys)) = (a.kind(), b.kind()) {
        return (xs.len() == ys.len())
            .then(|| xs.iter().zip(ys).map(|(a, b)| join(a, b)).collect::<Option<Vec<_>>>())
            .flatten()
            .map(Ty::tuple);
    }
    if super::constraint::is_numeric_scalar_ty(a) && super::constraint::is_numeric_scalar_ty(b) {
        return Some(Ty::named("int"));
    }
    let (TyKind::App { name: an, args: aa, .. } | TyKind::Named(an, aa)) = a.kind() else {
        return None;
    };
    let (TyKind::App { name: bn, args: ba, .. } | TyKind::Named(bn, ba)) = b.kind() else {
        return None;
    };
    if an != bn || aa.len() != ba.len() {
        return None;
    }
    let mut used = std::collections::HashSet::new();
    super::substitution::ty_names(a, &mut used, true);
    super::substitution::ty_names(b, &mut used, true);
    let mut vars = Vec::new();
    let mut facts = Vec::new();
    let mut args = Vec::new();
    for (a, b) in aa.iter().zip(ba) {
        if a == b {
            args.push(a.clone());
            continue;
        }
        let (TyArg::Nexp(a), TyArg::Nexp(b)) = (a, b) else { return None };
        if super::subtype::contains_erased_numeric(a) || super::subtype::contains_erased_numeric(b)
        {
            return None;
        }
        if super::numeric::numeric_equality_without_binding(a, b, &Subst::default(), &[])
            == ConstraintStatus::Satisfied
        {
            args.push(TyArg::Nexp(a.clone()));
            continue;
        }
        let mut index = vars.len();
        let name = loop {
            let n = format!("'_join{index}");
            if used.insert(n[1..].to_owned()) {
                break n;
            }
            index += 1;
        };
        let n = NumericExpr::Var(name.clone());
        facts.push(ConstraintExpr::Or(vec![
            ConstraintExpr::Compare { lhs: n.clone(), op: CompareOp::Eq, rhs: a.clone() },
            ConstraintExpr::Compare { lhs: n.clone(), op: CompareOp::Eq, rhs: b.clone() },
        ]));
        vars.push(name);
        args.push(TyArg::Nexp(n));
    }
    let inner = Ty::app(an, args, "");
    Some(if vars.is_empty() { inner } else { Ty::exist(vars, ConstraintExpr::And(facts), inner) })
}

pub(super) fn conditional_join(cond: &ConstraintExpr, a: &Ty, b: &Ty) -> Option<Ty> {
    if matches!(a.kind(), TyKind::Exist { .. }) || matches!(b.kind(), TyKind::Exist { .. }) {
        let mut used = HashSet::new();
        super::substitution::ty_names(a, &mut used, true);
        super::substitution::ty_names(b, &mut used, true);
        super::substitution::constraint_names(cond, &mut used);
        let mut vars = Vec::new();
        let mut facts = Vec::new();
        let mut unpack = |ty: &Ty, active: ConstraintExpr| {
            let mut ty = ty.clone();
            while let TyKind::Exist { vars: bound, constraint, inner } = ty.kind() {
                let mut subst = Subst::default();
                for var in bound {
                    let mut index = vars.len();
                    let name = loop {
                        let name = format!("'_branch{index}");
                        if used.insert(name[1..].to_owned()) {
                            break name;
                        }
                        index += 1;
                    };
                    subst.values.insert(var.clone(), name.clone());
                    subst.types.insert(var.clone(), Ty::param(&name));
                    vars.push(name);
                }
                // A branch's witnesses satisfy its predicate only when that
                // branch is selected. Do not assume both predicates at once.
                facts.push(ConstraintExpr::Or(vec![
                    ConstraintExpr::Not(Box::new(active.clone())),
                    super::constraint::apply_subst_constraint_expr(constraint, &subst),
                ]));
                ty = super::constraint::apply_subst(inner, &subst);
            }
            ty
        };
        let left = unpack(a, cond.clone());
        let right = unpack(b, ConstraintExpr::Not(Box::new(cond.clone())));
        let inner = conditional_join(cond, &left, &right)?;
        return Some(Ty::exist(vars, ConstraintExpr::And(facts), inner));
    }
    if let (Some(a), Some(b)) = (super::subtype::bool_value(a), super::subtype::bool_value(b)) {
        return Some(Ty::app(
            "bool",
            vec![TyArg::Bool(ConstraintExpr::Or(vec![
                ConstraintExpr::And(vec![cond.clone(), a]),
                ConstraintExpr::And(vec![ConstraintExpr::Not(Box::new(cond.clone())), b]),
            ]))],
            "",
        ));
    }

    if let (Some(a), Some(b)) = (singleton(a), singleton(b)) {
        return Some(Ty::app(
            "atom",
            vec![TyArg::Nexp(NumericExpr::If {
                cond: Box::new(cond.clone()),
                then_expr: Box::new(a),
                else_expr: Box::new(b),
            })],
            "",
        ));
    }
    if let (TyKind::Tuple(xs), TyKind::Tuple(ys)) = (a.kind(), b.kind()) {
        if xs.len() == ys.len() {
            return xs
                .iter()
                .zip(ys)
                .map(|(a, b)| conditional_join(cond, a, b).or_else(|| join(a, b)))
                .collect::<Option<Vec<_>>>()
                .map(Ty::tuple);
        }
    }
    None
}

pub(super) fn normalize(ty: &Ty, subst: &Subst, facts: &[ConstraintExpr]) -> Ty {
    match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) if !args.is_empty() => {
            let args = args
                .iter()
                .map(|a| match a {
                    TyArg::Nexp(n) => TyArg::Nexp(
                        super::integer::eval(n, subst, facts)
                            .map(NumericExpr::from_integer)
                            .unwrap_or_else(|| n.clone()),
                    ),
                    TyArg::Type(t) => TyArg::Type(normalize(t, subst, facts)),
                    _ => a.clone(),
                })
                .collect();
            Ty::app(name, args, "")
        }
        TyKind::Tuple(ts) => Ty::tuple(ts.iter().map(|t| normalize(t, subst, facts)).collect()),
        _ => ty.clone(),
    }
}

/// Express a range or natural number as a singleton with its bound predicate.
/// The caller opens this rigidly; it cannot pick a convenient domain member.
pub(super) fn numeric_value(ty: &Ty) -> Option<(String, ConstraintExpr, Ty)> {
    if matches!(ty.kind(), TyKind::Scalar(Scalar::Int)) {
        let var = "'_argument".to_owned();
        return Some((
            var.clone(),
            ConstraintExpr::Bool(true),
            Ty::app("atom", vec![TyArg::Nexp(NumericExpr::Var(var))], ""),
        ));
    }
    if matches!(ty.kind(), TyKind::Scalar(Scalar::Nat)) {
        let var = "'_argument".to_owned();
        let value = NumericExpr::Var(var.clone());
        return Some((
            var,
            ConstraintExpr::Compare {
                lhs: NumericExpr::Const(0),
                op: CompareOp::Lte,
                rhs: value.clone(),
            },
            Ty::app("atom", vec![TyArg::Nexp(value)], ""),
        ));
    }
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    if name != "range" || args.len() != 2 {
        return None;
    }
    // Legacy type lowering stores named numeric bounds as Type arguments.
    // Recover their numeric syntax, then reject erased primitive types below.
    let bound = |arg: &TyArg| match arg {
        TyArg::Type(ty) => NumericExpr::parse(&ty.display_text()),
        _ => NumericExpr::parse(&arg.as_value_str()?),
    };
    let lo = bound(&args[0])?;
    let hi = bound(&args[1])?;
    if super::subtype::contains_erased_numeric(&lo) || super::subtype::contains_erased_numeric(&hi)
    {
        return None;
    }
    let mut names = HashSet::new();
    super::substitution::ty_names(ty, &mut names, true);
    let mut var = "'_argument".to_owned();
    while names.contains(var.trim_start_matches('\'')) {
        var.push('_');
    }
    let value = NumericExpr::Var(var.clone());
    let predicate = ConstraintExpr::And(vec![
        ConstraintExpr::Compare { lhs: lo, op: CompareOp::Lte, rhs: value.clone() },
        ConstraintExpr::Compare { lhs: value.clone(), op: CompareOp::Lte, rhs: hi },
    ]);
    Some((var, predicate, Ty::app("atom", vec![TyArg::Nexp(value)], "")))
}

/// Interval consequences of a comparison against another bounded value.
/// Conditional min/max preserve existing bounds when symbolic endpoints
/// cannot be ordered without a solver.
pub(super) fn range_guard(
    body: &Body,
    id: ExprId,
    negated: bool,
    lookup: &impl Fn(&str) -> Option<Ty>,
) -> Option<(String, Ty)> {
    use NumericExpr as N;
    let Expr::BinaryOp { lhs, op, rhs } = body.expr(id)? else { return None };
    let Expr::Ident(name) = body.expr(*lhs)? else { return None };
    let Expr::Ident(other) = body.expr(*rhs)? else { return None };
    let bounds = |name: &str| {
        let ty = lookup(name)?;
        let TyKind::App { name, args, .. } = ty.kind() else { return None };
        if name != "range" || args.len() != 2 {
            return None;
        }
        let numeric = |arg: &TyArg| match arg {
            TyArg::Type(ty) => N::parse(&ty.display_text()),
            _ => N::parse(&arg.as_value_str()?),
        };
        Some((numeric(&args[0])?, numeric(&args[1])?))
    };
    let ((mut lo, mut hi), (other_lo, other_hi)) = (bounds(name)?, bounds(other)?);
    let (upper, strict) = match (op.as_str(), negated) {
        ("<=", false) | (">", true) => (true, false),
        ("<", false) | (">=", true) => (true, true),
        (">=", false) | ("<", true) => (false, false),
        (">", false) | ("<=", true) => (false, true),
        _ => return None,
    };
    let mut endpoint = if upper { other_hi } else { other_lo };
    if strict {
        endpoint = N::Add(Box::new(endpoint), Box::new(N::Const(if upper { -1 } else { 1 })));
    }
    let original = if upper { &hi } else { &lo };
    let chosen = N::If {
        cond: Box::new(ConstraintExpr::Compare {
            lhs: original.clone(),
            op: if upper { CompareOp::Lte } else { CompareOp::Gte },
            rhs: endpoint.clone(),
        }),
        then_expr: Box::new(original.clone()),
        else_expr: Box::new(endpoint),
    };
    if upper {
        hi = chosen
    } else {
        lo = chosen
    }
    Some((name.clone(), Ty::app("range", vec![TyArg::Nexp(lo), TyArg::Nexp(hi)], "")))
}
