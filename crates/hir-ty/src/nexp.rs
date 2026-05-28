//! Numeric type expressions for Sail's dependent type system.
//!
//! Mirrors the sail compiler's `nexp_aux` from `src/rocq/lib/Ast.v`:
//!
//! ```text
//! Inductive nexp_aux :=
//!   | Nexp_id          (* type abbreviation *)
//!   | Nexp_var         (* kinded type variable *)
//!   | Nexp_constant    (* integer literal *)
//!   | Nexp_app         (* function application *)
//!   | Nexp_times | Nexp_sum | Nexp_minus | Nexp_exp | Nexp_neg
//!   | Nexp_if          (* conditional *)
//! ```
//!
//! These represent the numeric expressions that appear in dependent
//! types like `bits(n)`, `range(lo, hi)`, and constraints like `n > 0`.
//!
//! This is Sail-specific — RA has no equivalent since Rust has no
//! dependent types.

use hir_def::name::Name;

/// A numeric expression in Sail's type system.
///
/// Examples:
///   - `8` → `Nexp::Constant(8)`
///   - `'n` → `Nexp::Var(Name("'n"))`
///   - `2 ^ 'n` → `Nexp::Exp(Box::new(Nexp::Var(...)))`
///   - `8 * 'n + 3` → `Nexp::Sum(Box::new(Nexp::Times(...)), Box::new(Nexp::Constant(3)))`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Nexp {
    /// Integer literal: `8`, `0`, `64`.
    Constant(i64),
    /// Type variable: `'n`, `'m`.
    Var(Name),
    /// Named type abbreviation: `xlen`, `flen`.
    Id(Name),
    /// Function application: `f(n1, n2, ...)`.
    App(Name, Vec<Nexp>),
    /// Multiplication: `n1 * n2`.
    Times(Box<Nexp>, Box<Nexp>),
    /// Addition: `n1 + n2`.
    Sum(Box<Nexp>, Box<Nexp>),
    /// Subtraction: `n1 - n2`.
    Minus(Box<Nexp>, Box<Nexp>),
    /// Exponentiation: `2 ^ n` (always base 2 in Sail).
    Exp(Box<Nexp>),
    /// Negation: `-n`.
    Neg(Box<Nexp>),
    /// Conditional: `if c then n1 else n2`.
    If(Box<NConstraint>, Box<Nexp>, Box<Nexp>),
}

/// A numeric constraint in Sail's type system.
///
/// Mirrors the sail compiler's `n_constraint_aux` from `Ast.v`:
///
/// ```text
/// Inductive n_constraint_aux :=
///   | NC_equal | NC_not_equal
///   | NC_ge | NC_gt | NC_le | NC_lt
///   | NC_set | NC_and | NC_or
///   | NC_app | NC_id | NC_var
///   | NC_true | NC_false
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NConstraint {
    /// `n1 == n2`
    Equal(Nexp, Nexp),
    /// `n1 != n2`
    NotEqual(Nexp, Nexp),
    /// `n1 >= n2`
    Ge(Nexp, Nexp),
    /// `n1 > n2`
    Gt(Nexp, Nexp),
    /// `n1 <= n2`
    Le(Nexp, Nexp),
    /// `n1 < n2`
    Lt(Nexp, Nexp),
    /// Set membership: `n in {1, 2, 4, 8}`
    Set(Nexp, Vec<i64>),
    /// Logical AND: `c1 & c2`
    And(Box<NConstraint>, Box<NConstraint>),
    /// Logical OR: `c1 | c2`
    Or(Box<NConstraint>, Box<NConstraint>),
    /// Constraint function application.
    App(Name, Vec<Nexp>),
    /// Named constraint variable.
    Var(Name),
    /// Always true.
    True,
    /// Always false.
    False,
}

/// Environment for evaluating numeric expressions (known bindings).
pub type NexpEnv = rustc_hash::FxHashMap<Name, i64>;

impl Nexp {
    /// Try to evaluate this expression to a concrete integer.
    ///
    /// Returns `None` if the expression contains free variables not
    /// in `env`, or if evaluation would be non-trivial (e.g., large
    /// exponentiation).
    pub fn eval(&self, env: &NexpEnv) -> Option<i64> {
        match self {
            Nexp::Constant(n) => Some(*n),
            Nexp::Var(name) | Nexp::Id(name) => env.get(name).copied(),
            Nexp::Times(a, b) => Some(a.eval(env)?.checked_mul(b.eval(env)?)?),
            Nexp::Sum(a, b) => Some(a.eval(env)?.checked_add(b.eval(env)?)?),
            Nexp::Minus(a, b) => Some(a.eval(env)?.checked_sub(b.eval(env)?)?),
            Nexp::Neg(a) => Some(a.eval(env)?.checked_neg()?),
            Nexp::Exp(a) => {
                let n = a.eval(env)?;
                if (0..=63).contains(&n) {
                    Some(1i64 << n)
                } else {
                    None // overflow guard
                }
            }
            Nexp::App(_, _) => None, // can't evaluate function applications
            Nexp::If(cond, then_val, else_val) => match cond.eval(env) {
                Some(true) => then_val.eval(env),
                Some(false) => else_val.eval(env),
                None => None,
            },
        }
    }

    /// Check if this is a simple constant.
    pub fn as_constant(&self) -> Option<i64> {
        match self {
            Nexp::Constant(n) => Some(*n),
            _ => None,
        }
    }

    /// Check if this is a simple variable.
    pub fn as_var(&self) -> Option<&Name> {
        match self {
            Nexp::Var(name) => Some(name),
            _ => None,
        }
    }
}

impl NConstraint {
    /// Try to evaluate this constraint to a boolean.
    ///
    /// Returns `None` if free variables prevent evaluation.
    pub fn eval(&self, env: &NexpEnv) -> Option<bool> {
        match self {
            NConstraint::Equal(a, b) => Some(a.eval(env)? == b.eval(env)?),
            NConstraint::NotEqual(a, b) => Some(a.eval(env)? != b.eval(env)?),
            NConstraint::Ge(a, b) => Some(a.eval(env)? >= b.eval(env)?),
            NConstraint::Gt(a, b) => Some(a.eval(env)? > b.eval(env)?),
            NConstraint::Le(a, b) => Some(a.eval(env)? <= b.eval(env)?),
            NConstraint::Lt(a, b) => Some(a.eval(env)? < b.eval(env)?),
            NConstraint::Set(n, set) => {
                let val = n.eval(env)?;
                Some(set.contains(&val))
            }
            NConstraint::And(a, b) => Some(a.eval(env)? && b.eval(env)?),
            NConstraint::Or(a, b) => Some(a.eval(env)? || b.eval(env)?),
            NConstraint::True => Some(true),
            NConstraint::False => Some(false),
            NConstraint::App(_, _) | NConstraint::Var(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(bindings: &[(&str, i64)]) -> NexpEnv {
        bindings.iter().map(|(k, v)| (Name::new(k), *v)).collect()
    }

    #[test]
    fn eval_constant() {
        let env = NexpEnv::default();
        assert_eq!(Nexp::Constant(42).eval(&env), Some(42));
    }

    #[test]
    fn eval_var() {
        let env = env_with(&[("'n", 8)]);
        assert_eq!(Nexp::Var(Name::new("'n")).eval(&env), Some(8));
    }

    #[test]
    fn eval_arithmetic() {
        let env = env_with(&[("'n", 4)]);
        // 8 * 'n + 3 = 35
        let expr = Nexp::Sum(
            Box::new(Nexp::Times(
                Box::new(Nexp::Constant(8)),
                Box::new(Nexp::Var(Name::new("'n"))),
            )),
            Box::new(Nexp::Constant(3)),
        );
        assert_eq!(expr.eval(&env), Some(35));
    }

    #[test]
    fn eval_exp() {
        let env = env_with(&[("'n", 5)]);
        // 2^'n = 32
        let expr = Nexp::Exp(Box::new(Nexp::Var(Name::new("'n"))));
        assert_eq!(expr.eval(&env), Some(32));
    }

    #[test]
    fn eval_exp_overflow_guard() {
        let env = env_with(&[("'n", 100)]);
        let expr = Nexp::Exp(Box::new(Nexp::Var(Name::new("'n"))));
        assert_eq!(expr.eval(&env), None); // too large
    }

    #[test]
    fn eval_free_var_returns_none() {
        let env = NexpEnv::default();
        assert_eq!(Nexp::Var(Name::new("'n")).eval(&env), None);
    }

    #[test]
    fn eval_constraint_equal() {
        let env = env_with(&[("'n", 8)]);
        let c = NConstraint::Equal(Nexp::Var(Name::new("'n")), Nexp::Constant(8));
        assert_eq!(c.eval(&env), Some(true));
    }

    #[test]
    fn eval_constraint_and() {
        let env = env_with(&[("'n", 8)]);
        let c = NConstraint::And(
            Box::new(NConstraint::Gt(Nexp::Var(Name::new("'n")), Nexp::Constant(0))),
            Box::new(NConstraint::Le(Nexp::Var(Name::new("'n")), Nexp::Constant(64))),
        );
        assert_eq!(c.eval(&env), Some(true));
    }

    #[test]
    fn eval_constraint_set() {
        let env = env_with(&[("'n", 4)]);
        let c = NConstraint::Set(Nexp::Var(Name::new("'n")), vec![1, 2, 4, 8, 16, 32, 64]);
        assert_eq!(c.eval(&env), Some(true));
    }
}
