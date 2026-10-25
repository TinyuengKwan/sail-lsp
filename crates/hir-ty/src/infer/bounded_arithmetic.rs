//! Bounded arithmetic on numeric types. No SMT calls or unbounded enumeration.
use super::*;
use hir_def::hir::ArithOp;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};

type Bounds = (Option<NumericExpr>, Option<NumericExpr>);
const MAX_VALUES: usize = 64;
const MAX_EXPONENT: u32 = 4096;

fn numeric(arg: &TyArg) -> Option<NumericExpr> {
    let n = match arg {
        TyArg::Type(t) => NumericExpr::parse(&t.display_text()),
        _ => NumericExpr::parse(&arg.as_value_str()?),
    }?;
    (!super::subtype::contains_erased_numeric(&n)).then_some(n)
}
fn bounds(ty: &Ty) -> Option<Bounds> {
    if let Some(n) = super::refinement::singleton(ty) {
        return (!super::subtype::contains_erased_numeric(&n)).then(|| (Some(n.clone()), Some(n)));
    }
    match ty.kind() {
        TyKind::Scalar(Scalar::Nat) => Some((Some(NumericExpr::Const(0)), None)),
        TyKind::Scalar(Scalar::Int) => Some((None, None)),
        TyKind::App { name, args, .. } if name == "range" && args.len() == 2 => {
            Some((Some(numeric(&args[0])?), Some(numeric(&args[1])?)))
        }
        _ => None,
    }
}
fn folded(n: NumericExpr) -> NumericExpr {
    super::nexp_simplify::simplify(&n)
}
fn add(a: NumericExpr, b: NumericExpr) -> NumericExpr {
    folded(NumericExpr::Add(Box::new(a), Box::new(b)))
}
fn sub(a: NumericExpr, b: NumericExpr) -> NumericExpr {
    folded(NumericExpr::Sub(Box::new(a), Box::new(b)))
}
fn mul(a: NumericExpr, b: NumericExpr) -> NumericExpr {
    folded(NumericExpr::Mul(Box::new(a), Box::new(b)))
}
fn eval(n: &NumericExpr) -> Option<BigInt> {
    super::integer::eval(n, &Subst::default(), &[])
}
fn range((lo, hi): Bounds) -> Option<Ty> {
    if let (Some(lo), Some(hi)) = (&lo, &hi) {
        return Some(Ty::app("range", vec![TyArg::Nexp(lo.clone()), TyArg::Nexp(hi.clone())], ""));
    }
    if lo.as_ref().is_some_and(|n| eval(n).is_some_and(|n| !n.is_negative())) && hi.is_none() {
        return Some(Ty::named("nat"));
    }
    None
}
fn scale((lo, hi): Bounds, n: BigInt) -> Bounds {
    if n.is_zero() {
        return (Some(NumericExpr::Const(0)), Some(NumericExpr::Const(0)));
    }
    let negative = n.is_negative();
    let n = NumericExpr::from_integer(n);
    let lo = lo.map(|v| mul(v, n.clone()));
    let hi = hi.map(|v| mul(v, n));
    if negative {
        (hi, lo)
    } else {
        (lo, hi)
    }
}

/// Includes only positive conjuncts: neither a disjunction nor a negated bound
/// establishes a domain for every evaluation of its operand.
fn conjuncts<'a>(facts: &'a [ConstraintExpr], out: &mut Vec<&'a ConstraintExpr>) {
    let mut pending = vec![facts.iter()];
    for _ in 0..256 {
        let Some(frame) = pending.last_mut() else { break };
        match frame.next() {
            Some(ConstraintExpr::And(xs)) => pending.push(xs.iter()),
            Some(fact) => out.push(fact),
            None => {
                pending.pop();
            }
        }
    }
}
fn finite_values(ty: &Ty, facts: &[ConstraintExpr], depth: usize) -> Option<Vec<BigInt>> {
    if depth > 8 {
        return None;
    }
    if let TyKind::Exist { constraint, inner, .. } = ty.kind() {
        // Existential names are local to this type. Do not mix them with facts
        // using the same spelling outside the package.
        return finite_values(inner, std::slice::from_ref(constraint), depth + 1);
    }
    let (mut lo, mut hi) = bounds(ty)?;
    if let Some(index) = super::refinement::singleton(ty) {
        if let Some(n) = eval(&index) {
            return Some(vec![n]);
        }
        lo = None;
        hi = None;
        let mut flat = Vec::new();
        conjuncts(facts, &mut flat);
        for c in flat {
            match c {
                ConstraintExpr::InSet { value, items }
                    if value == &index && items.len() <= MAX_VALUES =>
                {
                    if let Some(mut values) = items.iter().map(eval).collect::<Option<Vec<_>>>() {
                        values.sort();
                        values.dedup();
                        if !values.is_empty() {
                            return Some(values);
                        }
                    }
                }
                ConstraintExpr::Compare { lhs, op, rhs } => {
                    let (op, value) = if lhs == &index {
                        (*op, eval(rhs))
                    } else if rhs == &index {
                        (super::constraint::swapped_compare_op(*op), eval(lhs))
                    } else {
                        continue;
                    };
                    let Some(value) = value else { continue };
                    let (lower, upper) = match op {
                        CompareOp::Eq => (Some(value.clone()), Some(value)),
                        CompareOp::Gte => (Some(value), None),
                        CompareOp::Gt => (Some(value + 1), None),
                        CompareOp::Lte => (None, Some(value)),
                        CompareOp::Lt => (None, Some(value - 1)),
                        _ => (None, None),
                    };
                    if let Some(v) = lower {
                        lo = Some(NumericExpr::from_integer(
                            lo.as_ref().and_then(eval).map_or(v.clone(), |a| a.max(v)),
                        ));
                    }
                    if let Some(v) = upper {
                        hi = Some(NumericExpr::from_integer(
                            hi.as_ref().and_then(eval).map_or(v.clone(), |a| a.min(v)),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    let (lo, hi) = (eval(&lo?)?, eval(&hi?)?);
    let count = (&hi - &lo + BigInt::from(1)).to_usize()?;
    if count == 0 || count > MAX_VALUES {
        return None;
    }
    Some((0..count).map(|i| &lo + BigInt::from(i)).collect())
}

/// Exact finite domain established by positive membership or integer bounds.
pub(super) fn numeric_domain(
    value: &NumericExpr,
    subst: &Subst,
    facts: &[ConstraintExpr],
) -> Option<Vec<String>> {
    let value = super::constraint::subst_numeric_expr(value, subst);
    let mut flat = Vec::new();
    conjuncts(facts, &mut flat);
    let facts = flat
        .into_iter()
        .filter(|c| matches!(c, ConstraintExpr::Compare { .. } | ConstraintExpr::InSet { .. }))
        .map(|c| super::constraint::apply_subst_constraint_expr(c, subst))
        .collect::<Vec<_>>();
    finite_values(&Ty::app("atom", vec![TyArg::Nexp(value)], ""), &facts, 0)
        .map(|values| values.into_iter().map(|value| value.to_string()).collect())
}

fn powers(exponent: &Ty, facts: &[ConstraintExpr]) -> Option<Vec<BigInt>> {
    finite_values(exponent, facts, 0)?
        .into_iter()
        .map(|n| {
            let n = n.to_u32()?;
            (n <= MAX_EXPONENT).then(|| BigInt::from(1) << n as usize)
        })
        .collect()
}

/// Prove membership for a stable symbolic power without discarding its exact
/// identity in the inferred type. Mixed membership stays Unknown.
pub(super) fn power_membership(
    value: &NumericExpr,
    items: &[NumericExpr],
    subst: &Subst,
    facts: &[ConstraintExpr],
) -> Option<ConstraintStatus> {
    if !matches!(value, NumericExpr::Exp(_)) || items.len() > MAX_VALUES {
        return None;
    }
    let NumericExpr::Exp(exponent) = super::constraint::subst_numeric_expr(value, subst) else {
        return None;
    };
    let mut flat = Vec::new();
    conjuncts(facts, &mut flat);
    let facts = flat
        .into_iter()
        .filter(|c| matches!(c, ConstraintExpr::Compare { .. } | ConstraintExpr::InSet { .. }))
        .map(|c| super::constraint::apply_subst_constraint_expr(c, subst))
        .collect::<Vec<_>>();
    let values = powers(&Ty::app("atom", vec![TyArg::Nexp(*exponent)], ""), &facts)?;
    let members =
        items.iter().map(|n| super::integer::eval(n, subst, &[])).collect::<Option<Vec<_>>>()?;
    if values.iter().all(|n| members.contains(n)) {
        Some(ConstraintStatus::Satisfied)
    } else if values.iter().all(|n| !members.contains(n)) {
        Some(ConstraintStatus::Failed)
    } else {
        None
    }
}

pub(super) fn infer(op: &ArithOp, lhs: &Ty, rhs: &Ty, facts: &[ConstraintExpr]) -> Option<Ty> {
    if matches!(op, ArithOp::BitXor) {
        // Sail uses ^ for numeric exponentiation as well as bitvector XOR.
        // Only the numeric base 2 enters this path.
        if super::refinement::singleton(lhs).as_ref().and_then(eval)? != BigInt::from(2) {
            return None;
        }
        let mut powers =
            powers(rhs, facts)?.into_iter().map(NumericExpr::from_integer).collect::<Vec<_>>();
        if powers.len() == 1 {
            return Some(Ty::app("atom", vec![TyArg::Nexp(powers.remove(0))], ""));
        }
        let var = "'_power".to_owned();
        let index = NumericExpr::Var(var.clone());
        if let Some(exponent) = super::refinement::singleton(rhs) {
            // A stable operand already has an identity. Keep its exact power
            // so subsequent branch comparisons can relate it to that operand.
            return Some(Ty::app(
                "atom",
                vec![TyArg::Nexp(NumericExpr::Exp(Box::new(exponent)))],
                "",
            ));
        }
        return Some(Ty::exist(
            vec![var],
            ConstraintExpr::InSet { value: index.clone(), items: powers },
            Ty::app("atom", vec![TyArg::Nexp(index)], ""),
        ));
    }
    if !matches!(op, ArithOp::Add | ArithOp::Sub | ArithOp::Mul) {
        return None;
    }
    let (a, b) = bounds(lhs)?;
    let (c, d) = bounds(rhs)?;
    // Symbolic singletons already carry exact identity through overloads or
    // source arithmetic; do not widen that relation to an independent range.
    if super::refinement::singleton(lhs).is_some()
        && super::refinement::singleton(rhs).is_some()
        && (a.as_ref().and_then(eval).is_none() || c.as_ref().and_then(eval).is_none())
    {
        return None;
    }
    let result = match op {
        ArithOp::Add => (a.zip(c).map(|(a, c)| add(a, c)), b.zip(d).map(|(b, d)| add(b, d))),
        ArithOp::Sub => (a.zip(d).map(|(a, d)| sub(a, d)), b.zip(c).map(|(b, c)| sub(b, c))),
        ArithOp::Mul => {
            if c == d {
                if let Some(n) = c.as_ref().and_then(eval) {
                    return range(scale((a, b), n));
                }
            }
            if a == b {
                if let Some(n) = a.as_ref().and_then(eval) {
                    return range(scale((c, d), n));
                }
            }
            let (a, b, c, d) = (a?, b?, c?, d?);
            let corners = [
                mul(a.clone(), c.clone()),
                mul(a.clone(), d.clone()),
                mul(b.clone(), c.clone()),
                mul(b.clone(), d.clone()),
            ];
            if let Some(values) = corners.iter().map(eval).collect::<Option<Vec<_>>>() {
                (
                    Some(NumericExpr::from_integer(values.iter().min()?.clone())),
                    Some(NumericExpr::from_integer(values.iter().max()?.clone())),
                )
            } else {
                let nonnegative = |n: &NumericExpr| {
                    super::integer::condition(
                        &ConstraintExpr::Compare {
                            lhs: n.clone(),
                            op: CompareOp::Gte,
                            rhs: NumericExpr::Const(0),
                        },
                        &Subst::default(),
                        facts,
                    ) == Some(true)
                };
                if !nonnegative(&a) || !nonnegative(&c) {
                    return None;
                }
                (Some(mul(a, c)), Some(mul(b, d)))
            }
        }
        _ => unreachable!(),
    };
    range(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atom(n: &str) -> Ty {
        Ty::app("atom", vec![TyArg::numeric(n)], "")
    }
    fn interval(lo: &str, hi: &str) -> Ty {
        Ty::app("range", vec![TyArg::numeric(lo), TyArg::numeric(hi)], "")
    }
    fn member() -> ConstraintExpr {
        ConstraintExpr::InSet {
            value: NumericExpr::Var("'n".into()),
            items: vec![NumericExpr::Const(3), NumericExpr::Const(5)],
        }
    }
    #[test]
    fn exponent_budget_and_negative_values_fall_back() {
        for rhs in [
            interval("0", "64"),
            interval("-1", "2"),
            atom("4097"),
            Ty::named("int"),
            Ty::named("nat"),
        ] {
            assert!(infer(&ArithOp::BitXor, &atom("2"), &rhs, &[]).is_none(), "{rhs:?}");
        }
        assert!(infer(&ArithOp::BitXor, &atom("2"), &interval("0", "63"), &[]).is_some());
        assert!(infer(&ArithOp::BitXor, &atom("2"), &atom("4096"), &[]).is_some());
        assert!(infer(
            &ArithOp::BitXor,
            &Ty::app("bits", vec![TyArg::numeric("8")], ""),
            &Ty::app("bits", vec![TyArg::numeric("8")], ""),
            &[]
        )
        .is_none());
    }
    #[test]
    fn finite_domains_do_not_use_negated_or_disjunctive_facts() {
        let n = atom("'n");
        assert_eq!(finite_values(&n, &[member()], 0), Some(vec![3.into(), 5.into()]));
        for fact in [ConstraintExpr::Not(Box::new(member())), ConstraintExpr::Or(vec![member()])] {
            assert!(finite_values(&n, &[fact], 0).is_none());
        }
        let mut deep = member();
        for _ in 0..300 {
            deep = ConstraintExpr::And(vec![deep]);
        }
        assert!(finite_values(&n, &[deep], 0).is_none());
    }
    #[test]
    fn existential_domains_ignore_same_named_outer_facts() {
        let inner = Ty::exist(vec!["'n".into()], ConstraintExpr::And(vec![]), atom("'n"));
        assert!(finite_values(&inner, &[member()], 0).is_none());
        let packaged = Ty::exist(vec!["'n".into()], member(), atom("'n"));
        assert_eq!(finite_values(&packaged, &[], 0), Some(vec![3.into(), 5.into()]));
    }
    #[test]
    fn symbolic_power_membership_is_uniform_and_scope_safe() {
        let value = NumericExpr::Exp(Box::new(NumericExpr::Var("'n".into())));
        let check = |items: &[i64], facts: &[ConstraintExpr]| {
            power_membership(
                &value,
                &items.iter().copied().map(NumericExpr::Const).collect::<Vec<_>>(),
                &Subst::default(),
                facts,
            )
        };
        assert_eq!(check(&[8, 32], &[member()]), Some(ConstraintStatus::Satisfied));
        assert_eq!(check(&[16], &[member()]), Some(ConstraintStatus::Failed));
        assert_eq!(check(&[8], &[member()]), None);
        assert_eq!(check(&[8, 32], &[ConstraintExpr::Not(Box::new(member()))]), None);
        assert_eq!(check(&[8, 32], &[]), None);
    }
    #[test]
    fn zero_scaling_and_subtraction_keep_sound_bounds() {
        let zero = infer(&ArithOp::Mul, &Ty::named("int"), &atom("0"), &[]).unwrap();
        assert_eq!(finite_values(&zero, &[], 0), Some(vec![0.into()]));
        let difference =
            infer(&ArithOp::Sub, &interval("-3", "2"), &interval("-4", "5"), &[]).unwrap();
        assert_eq!(bounds(&difference), bounds(&interval("-8", "6")));
        assert!(infer(&ArithOp::Mul, &interval("'a", "'b"), &interval("'c", "'d"), &[]).is_none());
    }
}
