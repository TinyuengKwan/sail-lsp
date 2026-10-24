//! NumericExpr constant folding and algebraic simplification.

use crate::ty::NumericExpr;

/// Simplify a numeric expression by constant folding and algebraic rules.
pub(super) fn simplify(expr: &NumericExpr) -> NumericExpr {
    if matches!(
        expr,
        NumericExpr::Const(_)
            | NumericExpr::BigConst(_)
            | NumericExpr::Var(_)
            | NumericExpr::Symbol(_)
    ) {
        return expr.clone();
    }
    if let Some(n) = super::integer::eval(expr, &super::Subst::default(), &[]) {
        return NumericExpr::from_integer(n);
    }
    match expr {
        // Constant folding
        NumericExpr::Add(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) if x.checked_add(*y).is_some() => {
                    NumericExpr::Const(x.checked_add(*y).unwrap())
                }
                (_, NumericExpr::Const(0)) => a,
                (NumericExpr::Const(0), _) => b,
                _ => NumericExpr::Add(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Sub(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) if x.checked_sub(*y).is_some() => {
                    NumericExpr::Const(x.checked_sub(*y).unwrap())
                }
                (_, NumericExpr::Const(0)) => a,
                // 'n - 'n = 0
                _ if a == b => NumericExpr::Const(0),
                _ => NumericExpr::Sub(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Mul(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) if x.checked_mul(*y).is_some() => {
                    NumericExpr::Const(x.checked_mul(*y).unwrap())
                }
                (_, NumericExpr::Const(1)) => a,
                (NumericExpr::Const(1), _) => b,
                (_, NumericExpr::Const(0)) | (NumericExpr::Const(0), _) => NumericExpr::Const(0),
                _ => NumericExpr::Mul(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Neg(e) => {
            let e = simplify(e);
            match &e {
                NumericExpr::Const(n) if n.checked_neg().is_some() => {
                    NumericExpr::Const(n.checked_neg().unwrap())
                }
                NumericExpr::Neg(inner) => *inner.clone(),
                _ => NumericExpr::Neg(Box::new(e)),
            }
        }
        NumericExpr::Exp(e) => {
            let e = simplify(e);
            if let NumericExpr::Const(n) = &e {
                if *n >= 0 && *n < 63 {
                    return NumericExpr::Const(1i64 << n);
                }
            }
            NumericExpr::Exp(Box::new(e))
        }
        NumericExpr::Div(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y))
                    if x.checked_div_euclid(*y).is_some() =>
                {
                    NumericExpr::Const(x.checked_div_euclid(*y).unwrap())
                }
                _ => NumericExpr::Div(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Mod(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y))
                    if x.checked_rem_euclid(*y).is_some() =>
                {
                    NumericExpr::Const(x.checked_rem_euclid(*y).unwrap())
                }
                _ => NumericExpr::Mod(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::App { name, args } => {
            let simplified_args: Vec<NumericExpr> = args.iter().map(simplify).collect();
            // Try to evaluate known functions with constant args.
            if simplified_args.iter().all(|a| matches!(a, NumericExpr::Const(_))) {
                let vals: Vec<i64> = simplified_args
                    .iter()
                    .filter_map(|a| if let NumericExpr::Const(n) = a { Some(*n) } else { None })
                    .collect();
                match (name.as_str(), vals.as_slice()) {
                    ("div", [a, b]) if a.checked_div_euclid(*b).is_some() => {
                        return NumericExpr::Const(a.checked_div_euclid(*b).unwrap())
                    }
                    ("mod", [a, b]) if a.checked_rem_euclid(*b).is_some() => {
                        return NumericExpr::Const(a.checked_rem_euclid(*b).unwrap())
                    }
                    ("abs", [a]) if a.checked_abs().is_some() => {
                        return NumericExpr::Const(a.checked_abs().unwrap())
                    }
                    ("min", [a, b]) => return NumericExpr::Const(*a.min(b)),
                    ("max", [a, b]) => return NumericExpr::Const(*a.max(b)),
                    _ => {}
                }
            }
            NumericExpr::App { name: name.clone(), args: simplified_args }
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
            if let Some(value) =
                super::numeric::eval_numeric_expr(expr, &super::Subst::default(), &[])
            {
                return NumericExpr::Const(value);
            }
            let then_s = simplify(then_expr);
            let else_s = simplify(else_expr);
            // If both branches simplify to the same value, condition is irrelevant.
            if then_s == else_s {
                return then_s;
            }
            NumericExpr::If {
                cond: cond.clone(),
                then_expr: Box::new(then_s),
                else_expr: Box::new(else_s),
            }
        }
        // Leaves — already in simplest form
        other => other.clone(),
    }
}

/// Try to evaluate a NumericExpr to a constant.
pub(super) fn try_eval_const(expr: &NumericExpr) -> Option<i64> {
    match simplify(expr) {
        NumericExpr::Const(n) => Some(n),
        _ => None,
    }
}

/// Check if `a <= b` is provable by algebraic simplification.
///
/// Returns `Some(true)` if provably true, `Some(false)` if provably false,
/// `None` if undecidable without an SMT solver.
pub(super) fn check_le(a: &NumericExpr, b: &NumericExpr) -> Option<bool> {
    // Identity: a <= a is always true.
    if a == b {
        return Some(true);
    }
    // Simplify b - a; if result >= 0, then a <= b.
    let diff = simplify(&NumericExpr::Sub(Box::new(b.clone()), Box::new(a.clone())));
    match &diff {
        NumericExpr::Const(n) => return Some(*n >= 0),
        // Var('n) where we know nothing → undecidable.
        // But Add(Var('n), Const(k)) with k >= 0 means b - a >= 0 when k >= 0.
        NumericExpr::Var(_) => {}
        // b - a = expr + positive_const → might be provable if expr is non-negative.
        _ => {}
    }
    // Heuristic: 0 <= 'n and 'n <= max patterns.
    // 0 <= Var('n): common in Sail (sizes are non-negative), but we can't
    // assume it without a constraint. Return None.
    None
}

/// Check if `a == b` is provable by algebraic simplification.
///
/// Returns `Some(true)` if provably equal, `Some(false)` if provably different,
/// `None` if undecidable.
pub(super) fn check_eq(a: &NumericExpr, b: &NumericExpr) -> Option<bool> {
    if a == b {
        return Some(true);
    }
    let diff = simplify(&NumericExpr::Sub(Box::new(a.clone()), Box::new(b.clone())));
    match diff {
        NumericExpr::Const(0) => Some(true),
        NumericExpr::Const(_) => Some(false),
        // `8 * 'n == 65` has no integer solution, whatever `'n` is: a
        // width that is not a multiple of the stride cannot be reached.
        // Sail's variables are integers, so this is a proof and not a
        // heuristic — upstream proves the same thing with Z3.
        ref other => match as_linear(other) {
            // The variable cancelled: what is left is a constant, and the
            // equation holds exactly when it is zero.
            Some((0, _, c)) => Some(c == 0),
            Some((k, _, c)) if k != 0 && c.checked_rem(k).is_some_and(|r| r != 0) => Some(false),
            _ => None,
        },
    }
}

/// `expr` as `coefficient * variable + constant`, when it is linear in
/// exactly one variable.
///
/// `None` for anything else: two variables, a variable under `2^`, a
/// division. Those are the constraint solver's, not this.
pub(super) fn as_linear(expr: &NumericExpr) -> Option<(i64, String, i64)> {
    match expr {
        NumericExpr::Var(v) | NumericExpr::Symbol(v) => Some((1, v.clone(), 0)),
        NumericExpr::Const(_) => None,
        NumericExpr::Neg(inner) => {
            let (a, v, b) = as_linear(inner)?;
            Some((a.checked_neg()?, v, b.checked_neg()?))
        }
        NumericExpr::Add(l, r) | NumericExpr::Sub(l, r) => {
            let sign: i64 = if matches!(expr, NumericExpr::Sub(..)) { -1 } else { 1 };
            match (as_linear(l), as_linear(r)) {
                (Some((a, v, b)), None) => {
                    Some((a, v, b.checked_add(sign.checked_mul(try_eval_const(r)?)?)?))
                }
                (None, Some((a, v, b))) => Some((
                    sign.checked_mul(a)?,
                    v,
                    try_eval_const(l)?.checked_add(sign.checked_mul(b)?)?,
                )),
                // Both sides linear in the *same* variable: combine them.
                // Without this `2 + ('N - 1) - 'N` stayed a tree, so a
                // concatenation whose parts add up to one bit more than the
                // width it is checked against — `'N + 1 == 'N` — could not
                // be disproved by arithmetic that plainly disproves it.
                (Some((a1, v1, b1)), Some((a2, v2, b2))) if v1 == v2 => Some((
                    a1.checked_add(sign.checked_mul(a2)?)?,
                    v1,
                    b1.checked_add(sign.checked_mul(b2)?)?,
                )),
                _ => None,
            }
        }
        NumericExpr::Mul(l, r) => match (as_linear(l), as_linear(r)) {
            (Some((a, v, b)), None) => {
                let k = try_eval_const(r)?;
                Some((a.checked_mul(k)?, v, b.checked_mul(k)?))
            }
            (None, Some((a, v, b))) => {
                let k = try_eval_const(l)?;
                Some((a.checked_mul(k)?, v, b.checked_mul(k)?))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Solve `expr == value` for the one variable `expr` is linear in.
///
/// `Some(Ok((var, n)))` when the variable must be `n`; `Some(Err(()))` when
/// no integer satisfies it — `8 * 'n == 65` has no solution and the program
/// is wrong. `None` when this cannot tell.
pub(super) fn solve_linear(expr: &NumericExpr, value: i64) -> Option<Result<(String, i64), ()>> {
    let (a, var, b) = as_linear(&simplify(expr))?;
    if a == 0 {
        return None;
    }
    let rhs = value.checked_sub(b)?;
    if rhs.checked_rem(a)? != 0 {
        return Some(Err(()));
    }
    Some(Ok((var, rhs.checked_div(a)?)))
}

/// Evaluate an expression that is constant *after* like terms cancel.
///
/// `simplify` does not cancel: `'n + (64 - 'n)` stays a tree with `'n` in it
/// and `try_eval_const` gives up. Summing coefficients does the job, which is
/// all a concatenation's total width needs — every part contributes linearly.
pub(super) fn try_eval_linear_const(expr: &NumericExpr) -> Option<i64> {
    use std::collections::HashMap;
    fn go(
        e: &NumericExpr,
        sign: i64,
        vars: &mut HashMap<String, i64>,
        constant: &mut i64,
    ) -> Option<()> {
        match e {
            NumericExpr::Const(n) => {
                *constant = constant.checked_add(sign.checked_mul(*n)?)?;
                Some(())
            }
            NumericExpr::Var(v) | NumericExpr::Symbol(v) => {
                *vars.entry(v.clone()).or_insert(0) += sign;
                Some(())
            }
            NumericExpr::Neg(i) => go(i, -sign, vars, constant),
            NumericExpr::Add(a, b) => {
                go(a, sign, vars, constant)?;
                go(b, sign, vars, constant)
            }
            NumericExpr::Sub(a, b) => {
                go(a, sign, vars, constant)?;
                go(b, -sign, vars, constant)
            }
            NumericExpr::Mul(a, b) => match (try_eval_const(a), try_eval_const(b)) {
                (Some(k), None) => go(b, sign.checked_mul(k)?, vars, constant),
                (None, Some(k)) => go(a, sign.checked_mul(k)?, vars, constant),
                (Some(x), Some(y)) => {
                    *constant = constant.checked_add(sign.checked_mul(x.checked_mul(y)?)?)?;
                    Some(())
                }
                (None, None) => None,
            },
            _ => None,
        }
    }
    let mut vars = HashMap::new();
    let mut constant = 0i64;
    go(expr, 1, &mut vars, &mut constant)?;
    vars.values().all(|c| *c == 0).then_some(constant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::NumericExpr;

    fn c(n: i64) -> NumericExpr {
        NumericExpr::Const(n)
    }
    fn v(s: &str) -> NumericExpr {
        NumericExpr::Var(s.to_string())
    }
    fn add(a: NumericExpr, b: NumericExpr) -> NumericExpr {
        NumericExpr::Add(Box::new(a), Box::new(b))
    }
    fn sub(a: NumericExpr, b: NumericExpr) -> NumericExpr {
        NumericExpr::Sub(Box::new(a), Box::new(b))
    }
    fn mul(a: NumericExpr, b: NumericExpr) -> NumericExpr {
        NumericExpr::Mul(Box::new(a), Box::new(b))
    }
    fn neg(a: NumericExpr) -> NumericExpr {
        NumericExpr::Neg(Box::new(a))
    }
    fn exp(a: NumericExpr) -> NumericExpr {
        NumericExpr::Exp(Box::new(a))
    }
    fn div(a: NumericExpr, b: NumericExpr) -> NumericExpr {
        NumericExpr::Div(Box::new(a), Box::new(b))
    }
    fn modulo(a: NumericExpr, b: NumericExpr) -> NumericExpr {
        NumericExpr::Mod(Box::new(a), Box::new(b))
    }

    // --- simplify: constant folding ---

    #[test]
    fn test_add_constants() {
        assert_eq!(simplify(&add(c(3), c(5))), c(8));
    }

    #[test]
    fn test_add_zero_right() {
        assert_eq!(simplify(&add(v("n"), c(0))), v("n"));
    }

    #[test]
    fn test_add_zero_left() {
        assert_eq!(simplify(&add(c(0), v("n"))), v("n"));
    }

    #[test]
    fn test_sub_constants() {
        assert_eq!(simplify(&sub(c(10), c(3))), c(7));
    }

    #[test]
    fn test_sub_zero() {
        assert_eq!(simplify(&sub(v("n"), c(0))), v("n"));
    }

    #[test]
    fn test_sub_self() {
        assert_eq!(simplify(&sub(v("n"), v("n"))), c(0));
    }

    #[test]
    fn test_mul_constants() {
        assert_eq!(simplify(&mul(c(4), c(5))), c(20));
    }

    #[test]
    fn test_mul_one_right() {
        assert_eq!(simplify(&mul(v("n"), c(1))), v("n"));
    }

    #[test]
    fn test_mul_one_left() {
        assert_eq!(simplify(&mul(c(1), v("n"))), v("n"));
    }

    #[test]
    fn test_mul_zero_right() {
        assert_eq!(simplify(&mul(v("n"), c(0))), c(0));
    }

    #[test]
    fn test_mul_zero_left() {
        assert_eq!(simplify(&mul(c(0), v("n"))), c(0));
    }

    #[test]
    fn test_neg_constant() {
        assert_eq!(simplify(&neg(c(5))), c(-5));
    }

    #[test]
    fn test_neg_neg_cancel() {
        assert_eq!(simplify(&neg(neg(v("n")))), v("n"));
    }

    #[test]
    fn test_exp_constant() {
        // 2^8 = 256
        assert_eq!(simplify(&exp(c(8))), c(256));
    }

    #[test]
    fn test_exp_zero() {
        // 2^0 = 1
        assert_eq!(simplify(&exp(c(0))), c(1));
    }

    #[test]
    fn test_exp_63() {
        assert_eq!(
            simplify(&exp(c(63))),
            NumericExpr::integer_literal("9223372036854775808").unwrap()
        );
    }

    #[test]
    fn test_exp_variable_unchanged() {
        let e = exp(v("n"));
        assert_eq!(simplify(&e), e);
    }

    #[test]
    fn test_div_constants() {
        assert_eq!(simplify(&div(c(10), c(2))), c(5));
    }

    #[test]
    fn test_div_by_zero_unchanged() {
        let e = div(c(10), c(0));
        assert_eq!(simplify(&e), e);
    }

    #[test]
    fn test_mod_constants() {
        assert_eq!(simplify(&modulo(c(10), c(3))), c(1));
    }

    #[test]
    fn test_nested_simplification() {
        // (3 + 5) * (2 - 2) => 8 * 0 => 0
        let expr = mul(add(c(3), c(5)), sub(c(2), c(2)));
        assert_eq!(simplify(&expr), c(0));
    }

    #[test]
    fn test_var_unchanged() {
        assert_eq!(simplify(&v("n")), v("n"));
    }

    // --- try_eval_const ---

    #[test]
    fn test_try_eval_const_known() {
        assert_eq!(try_eval_const(&add(c(1), c(2))), Some(3));
    }

    #[test]
    fn test_try_eval_const_unknown() {
        assert_eq!(try_eval_const(&v("n")), None);
    }

    #[test]
    fn test_try_eval_const_partial() {
        assert_eq!(try_eval_const(&add(v("n"), c(0))), None);
    }

    // --- check_le ---

    #[test]
    fn test_check_le_true() {
        assert_eq!(check_le(&c(3), &c(5)), Some(true));
    }

    #[test]
    fn test_check_le_equal() {
        assert_eq!(check_le(&c(5), &c(5)), Some(true));
    }

    #[test]
    fn test_check_le_false() {
        assert_eq!(check_le(&c(7), &c(3)), Some(false));
    }

    #[test]
    fn test_check_le_variable_unknown() {
        assert_eq!(check_le(&v("n"), &v("m")), None);
    }

    #[test]
    fn test_check_le_same_var() {
        // n <= n is always true (b - a = n - n = 0 >= 0)
        assert_eq!(check_le(&v("n"), &v("n")), Some(true));
    }

    // --- check_eq ---

    #[test]
    fn test_check_eq_same_const() {
        assert_eq!(check_eq(&c(5), &c(5)), Some(true));
    }

    #[test]
    fn test_check_eq_different_const() {
        assert_eq!(check_eq(&c(5), &c(6)), Some(false));
    }

    #[test]
    fn test_check_eq_same_var() {
        assert_eq!(check_eq(&v("n"), &v("n")), Some(true));
    }

    #[test]
    fn test_check_eq_different_var() {
        assert_eq!(check_eq(&v("n"), &v("m")), None);
    }

    #[test]
    fn test_check_eq_expr_reduces() {
        // (n + 3) - (n + 3) = 0
        let expr = add(v("n"), c(3));
        assert_eq!(check_eq(&expr, &expr), Some(true));
    }
}

#[cfg(test)]
mod linear_tests {
    use crate::ty::NumericExpr as N;

    /// Two linear forms in the same variable combine.
    ///
    /// `2 + ('N - 1)` against `'N` is `'N + 1 == 'N`, which is false by
    /// arithmetic — but `as_linear` handled only "one side linear, the
    /// other constant", so the difference stayed a tree and `check_eq`
    /// answered "undecidable" to a plainly decidable question.
    #[test]
    fn two_linear_forms_in_one_variable_combine() {
        let a = N::parse("(2) + (('N - 1))").expect("parses");
        let b = N::parse("'N").expect("parses");
        assert_eq!(super::check_eq(&a, &b), Some(false));

        let exact = N::parse("(2) + (('N - 2))").expect("parses");
        assert_eq!(super::check_eq(&exact, &b), Some(true));
    }

    /// A width that is not a multiple of the stride has no integer solution.
    #[test]
    fn a_non_multiple_width_has_no_solution() {
        let a = N::parse("8 * 'n").expect("parses");
        assert_eq!(super::check_eq(&a, &N::Const(65)), Some(false));
        assert_eq!(super::check_eq(&a, &N::Const(64)), None);
    }
}
