//! NumericExpr constant folding and algebraic simplification.

use crate::ty::NumericExpr;

/// Simplify a numeric expression by constant folding and algebraic rules.
pub(super) fn simplify(expr: &NumericExpr) -> NumericExpr {
    match expr {
        // Constant folding
        NumericExpr::Add(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) => NumericExpr::Const(x + y),
                (_, NumericExpr::Const(0)) => a,
                (NumericExpr::Const(0), _) => b,
                _ => NumericExpr::Add(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Sub(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) => NumericExpr::Const(x - y),
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
                (NumericExpr::Const(x), NumericExpr::Const(y)) => NumericExpr::Const(x * y),
                (_, NumericExpr::Const(1)) => a,
                (NumericExpr::Const(1), _) => b,
                (_, NumericExpr::Const(0)) | (NumericExpr::Const(0), _) => NumericExpr::Const(0),
                _ => NumericExpr::Mul(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Neg(e) => {
            let e = simplify(e);
            match &e {
                NumericExpr::Const(n) => NumericExpr::Const(-n),
                NumericExpr::Neg(inner) => *inner.clone(),
                _ => NumericExpr::Neg(Box::new(e)),
            }
        }
        NumericExpr::Exp(e) => {
            let e = simplify(e);
            if let NumericExpr::Const(n) = &e {
                if *n >= 0 && *n <= 63 {
                    return NumericExpr::Const(1i64 << n);
                }
            }
            NumericExpr::Exp(Box::new(e))
        }
        NumericExpr::Div(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) if *y != 0 => {
                    NumericExpr::Const(x / y)
                }
                _ => NumericExpr::Div(Box::new(a), Box::new(b)),
            }
        }
        NumericExpr::Mod(a, b) => {
            let a = simplify(a);
            let b = simplify(b);
            match (&a, &b) {
                (NumericExpr::Const(x), NumericExpr::Const(y)) if *y != 0 => {
                    NumericExpr::Const(x % y)
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
                    ("div", [a, b]) if *b != 0 => return NumericExpr::Const(a / b),
                    ("mod", [a, b]) if *b != 0 => return NumericExpr::Const(a % b),
                    ("abs", [a]) => return NumericExpr::Const(a.abs()),
                    ("min", [a, b]) => return NumericExpr::Const(*a.min(b)),
                    ("max", [a, b]) => return NumericExpr::Const(*a.max(b)),
                    _ => {}
                }
            }
            NumericExpr::App { name: name.clone(), args: simplified_args }
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
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
        _ => None,
    }
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
        // largest safe shift
        assert_eq!(simplify(&exp(c(63))), c(1i64 << 63));
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
