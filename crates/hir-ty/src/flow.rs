//! Flow typing — guard-based type narrowing for Sail.
//!
//! Mirrors the Sail compiler's flow typing: when a guard expression
//! constrains the type of a variable, the types inside the guarded
//! branch are narrowed accordingly.
//!
//! Example:
//! ```sail
//! match x {
//!     _ if x == 0 => ...      // x narrowed to atom(0)
//!     _ if x > 0  => ...      // x narrowed to range(1, ...)
//!     _ if is_zero(x) => ...  // x narrowed via predicate
//! }
//! ```
//!
//! # Architecture
//!
//! Sail-specific (RA has no direct equivalent — Rust uses pattern
//! exhaustiveness + if-let chains, not numeric guard-based narrowing).
//!
//! The flow typing system:
//! 1. Analyzes guard expressions to extract constraints
//! 2. Maps constraints to type narrowings for specific variables
//! 3. Applies narrowings during arm body inference via `FlowEnvironment`

use crate::ty::{Ty, TyArg};
use hir_def::name::Name;
use hir_def::{Body, Expr, ExprId};
use rustc_hash::FxHashMap;

/// Type narrowing environment accumulated from guard expressions.
///
/// When entering a match arm with a guard `if cond`, we analyze `cond`
/// to extract type constraints (e.g., `x > 0` narrows `x` from `int`
/// to `range(1, max)`). These narrowed types are stored here and used
/// during inference of the arm body.
#[derive(Debug, Clone, Default)]
pub struct FlowEnvironment {
    /// Variable → narrowed type. Each entry overrides the type from
    /// the enclosing scope for the duration of the guarded branch.
    narrowed: FxHashMap<Name, Ty>,
}

impl FlowEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a type narrowing for `name`.
    pub fn narrow(&mut self, name: Name, ty: Ty) {
        self.narrowed.insert(name, ty);
    }

    /// Look up the narrowed type for `name`, if any.
    pub fn get(&self, name: &Name) -> Option<&Ty> {
        self.narrowed.get(name)
    }

    /// Check if this environment has any narrowings.
    pub fn is_empty(&self) -> bool {
        self.narrowed.is_empty()
    }

    /// Merge another FlowEnvironment into this one (AND semantics).
    /// Later narrowings override earlier ones.
    pub fn merge_and(&mut self, other: &FlowEnvironment) {
        for (name, ty) in &other.narrowed {
            self.narrowed.insert(name.clone(), ty.clone());
        }
    }

    /// Merge with OR semantics: keep only narrowings that appear in BOTH.
    /// Used for `cond1 || cond2` — a variable is only narrowed if both
    /// branches agree on the narrowing.
    pub fn merge_or(&mut self, other: &FlowEnvironment) {
        self.narrowed
            .retain(|name, ty| other.narrowed.get(name).is_some_and(|other_ty| other_ty == ty));
    }

    /// Number of narrowed bindings.
    pub fn len(&self) -> usize {
        self.narrowed.len()
    }

    /// Iterate all narrowed bindings.
    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Ty)> {
        self.narrowed.iter()
    }
}

/// Analyze a guard expression and extract type narrowings.
///
/// Handles common guard patterns:
///   - `x == constant` → narrows x to `atom(constant)`
///   - `x != constant` → no narrowing (can't negate to a type)
///   - `x > 0` → narrows x to `range(1, ...)`
///   - `x >= 0` → narrows x to `nat` (non-negative)
/// - `x < N` → narrows x to `range(..., )`
///   - `cond1 & cond2` → merge narrowings (AND)
///   - `f(x)` where f is a predicate → (future: predicate-based)
///
/// Returns a `FlowEnvironment` with the extracted narrowings.
/// Empty environment means no narrowings could be extracted.
pub fn narrow_from_guard(body: &Body, guard_expr: ExprId) -> FlowEnvironment {
    let mut env = FlowEnvironment::new();
    extract_narrowings(body, guard_expr, &mut env);
    env
}

/// Recursively extract narrowings from a guard expression.
fn extract_narrowings(body: &Body, expr_id: ExprId, env: &mut FlowEnvironment) {
    use hir_def::hir::{BinaryOp, CmpOp, HirBinaryOp, HirUnaryOp, LogicOp, UnaryOp};
    let expr = &body.store[expr_id];
    match expr {
        // Pattern: `x == constant` or `constant == x`
        Expr::BinaryOp { lhs, op, rhs }
            if matches!(op, HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Eq { negated: false })))
                || op.as_str() == "=" =>
        {
            try_extract_eq_narrowing(body, *lhs, *rhs, env);
        }

        // Pattern: `x != constant` or `constant != x`
        // Negated equality — narrow x to exclude the constant value.
        // While we can't express "not atom(N)" as a type, we can narrow
        // `x != 0` when x is known to be nat → range(1, max_int).
        Expr::BinaryOp { lhs, op, rhs }
            if matches!(op, HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Eq { negated: true })))
                || op.as_str() == "!=" =>
        {
            try_extract_neq_narrowing(body, *lhs, *rhs, env);
        }

        // Pattern: `x > 0`, `x >= 0`, `x < N`, `x <= N`
        Expr::BinaryOp { lhs, op, rhs }
            if matches!(op, HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord { .. })))
                || op.as_str() == ">_s"
                || op.as_str() == "<_s" =>
        {
            let op_str = op.as_str().to_string();
            try_extract_comparison_narrowing(body, *lhs, &op_str, *rhs, env);
        }

        // Pattern: `cond1 & cond2` (logical AND)
        Expr::BinaryOp { lhs, op, rhs }
            if matches!(op, HirBinaryOp::Known(BinaryOp::LogicOp(LogicOp::And)))
                || op.as_str() == "&&"
                || op.as_str() == "and" =>
        {
            // Both conditions must hold → merge narrowings
            extract_narrowings(body, *lhs, env);
            extract_narrowings(body, *rhs, env);
        }

        // Pattern: `cond1 | cond2` (logical OR)
        Expr::BinaryOp { lhs, op, rhs }
            if matches!(op, HirBinaryOp::Known(BinaryOp::LogicOp(LogicOp::Or)))
                || op.as_str() == "||"
                || op.as_str() == "or" =>
        {
            // Either condition holds → only keep common narrowings
            let mut env_left = FlowEnvironment::new();
            let mut env_right = FlowEnvironment::new();
            extract_narrowings(body, *lhs, &mut env_left);
            extract_narrowings(body, *rhs, &mut env_right);
            env_left.merge_or(&env_right);
            env.merge_and(&env_left);
        }

        // Pattern: `not(cond)` / `~(cond)` — negation (limited narrowing)
        Expr::UnaryOp { op, expr: inner } if matches!(op, HirUnaryOp::Known(UnaryOp::Not)) => {
            // Negation doesn't produce useful narrowings in most cases.
            // Future: for `not(x == 0)` we could narrow x to non-zero.
            let _ = inner;
        }

        // Pattern: function call `is_zero(x)`, `unsigned(x)` etc.
        // Future: predicate-based narrowing
        Expr::Call { callee, args } => {
            try_extract_predicate_narrowing(body, *callee, args, env);
        }

        // Anything else: no narrowing
        _ => {}
    }
}

/// Try to extract narrowing from `lhs != rhs` or `rhs != lhs`.
///
/// When `x != 0`, we can narrow x to `range(1, max_int)` (non-zero).
/// When `x != N` for other constants, the narrowing is less useful
/// (we'd need a "not-equal" type), so we only handle the common `!= 0` case.
fn try_extract_neq_narrowing(body: &Body, lhs: ExprId, rhs: ExprId, env: &mut FlowEnvironment) {
    // Case 1: `x != literal`
    if let Some((name, ty)) = ident_neq_literal(body, lhs, rhs) {
        env.narrow(Name::new(&name), ty);
        return;
    }
    // Case 2: `literal != x` (reversed)
    if let Some((name, ty)) = ident_neq_literal(body, rhs, lhs) {
        env.narrow(Name::new(&name), ty);
    }
}

/// Check if one side is an identifier and the other is a literal for != narrowing.
/// Returns (ident_name, narrowed_type) only for cases where useful narrowing exists.
fn ident_neq_literal(body: &Body, ident_side: ExprId, lit_side: ExprId) -> Option<(String, Ty)> {
    let name = match &body.store[ident_side] {
        Expr::Ident(n) => n.clone(),
        _ => return None,
    };
    match &body.store[lit_side] {
        Expr::Literal(parser::Literal::Number(n)) => {
            if let Ok(val) = n.parse::<i64>() {
                if val == 0 {
                    // x != 0 → range(1, max_int) (non-zero positive, or int minus 0)
                    // Common pattern in Sail: `if x != 0 then ...`
                    return Some((
                        name,
                        Ty::app(
                            "range",
                            vec![
                                TyArg::numeric("1"),
                                TyArg::numeric("max_int"),
                            ],
                            "range(1, max_int)".to_string(),
                        ),
                    ));
                }
                // For other constants (x != 5), no useful single-type narrowing
            }
            None
        }
        Expr::Literal(parser::Literal::Bool(b)) => {
            // x != true → narrows to false, x != false → narrows to true
            Some((name, Ty::named(if *b { "false" } else { "true" })))
        }
        _ => None,
    }
}

/// Try to extract narrowing from `lhs == rhs` or `rhs == lhs`.
fn try_extract_eq_narrowing(body: &Body, lhs: ExprId, rhs: ExprId, env: &mut FlowEnvironment) {
    // Case 1: `x == literal` → narrow x to atom(literal)
    if let Some((name, lit_ty)) = ident_eq_literal(body, lhs, rhs) {
        env.narrow(Name::new(&name), lit_ty);
        return;
    }
    // Case 2: `literal == x` (reversed)
    if let Some((name, lit_ty)) = ident_eq_literal(body, rhs, lhs) {
        env.narrow(Name::new(&name), lit_ty);
    }
}

/// Check if one side is an identifier and the other is a literal.
/// Returns (ident_name, narrowed_type).
fn ident_eq_literal(body: &Body, ident_side: ExprId, lit_side: ExprId) -> Option<(String, Ty)> {
    let name = match &body.store[ident_side] {
        Expr::Ident(n) => n.clone(),
        _ => return None,
    };
    let ty = match &body.store[lit_side] {
        Expr::Literal(parser::Literal::Number(n)) => {
            // Narrow to atom(N)
            Ty::app("atom", vec![TyArg::numeric(n.clone())], format!("atom({})", n))
        }
        Expr::Literal(parser::Literal::Bool(b)) => {
            // Narrow to atom_bool(true/false)
            Ty::named(if *b { "true" } else { "false" })
        }
        _ => return None,
    };
    Some((name, ty))
}

/// Try to extract narrowing from comparison operators.
fn try_extract_comparison_narrowing(
    body: &Body,
    lhs: ExprId,
    op: &str,
    rhs: ExprId,
    env: &mut FlowEnvironment,
) {
    // Pattern: `x > 0` or `x >= 0` → narrow x to nat
    if let Expr::Ident(name) = &body.store[lhs] {
        if let Expr::Literal(parser::Literal::Number(n)) = &body.store[rhs] {
            if let Ok(val) = n.parse::<i64>() {
                let narrowed = match op {
                    ">=" if val == 0 => Some(Ty::named("nat")),
                    ">" if val == 0 => {
                        // x > 0 → range(1, max_int) ≈ nat (close enough)
                        Some(Ty::app(
                            "range",
                            vec![
                                TyArg::numeric("1"),
                                TyArg::numeric("max_int"),
                            ],
                            "range(1, max_int)".to_string(),
                        ))
                    }
                    ">=" => Some(Ty::app(
                        "range",
                        vec![TyArg::numeric(val.to_string()), TyArg::numeric("max_int")],
                        format!("range({}, max_int)", val),
                    )),
                    "<" => Some(Ty::app(
                        "range",
                        vec![
                            TyArg::numeric("min_int"),
                            TyArg::numeric((val - 1).to_string()),
                        ],
                        format!("range(min_int, {})", val - 1),
                    )),
                    "<=" => Some(Ty::app(
                        "range",
                        vec![TyArg::numeric("min_int"), TyArg::numeric(val.to_string())],
                        format!("range(min_int, {})", val),
                    )),
                    _ => None,
                };
                if let Some(ty) = narrowed {
                    env.narrow(Name::new(name), ty);
                }
            }
        }
    }
}

/// Try to extract narrowing from predicate function calls.
/// Handles common Sail predicates:
///   - `is_zero(x)` → narrows x to atom(0)
///   - `unsigned(x)` → narrows x to nat
fn try_extract_predicate_narrowing(
    body: &Body,
    callee: ExprId,
    args: &[ExprId],
    env: &mut FlowEnvironment,
) {
    // Get predicate name
    let pred_name = match &body.store[callee] {
        Expr::Ident(n) => n.as_str(),
        _ => return,
    };

    // Must have exactly one argument that's an identifier
    if args.len() != 1 {
        return;
    }
    let arg_name = match &body.store[args[0]] {
        Expr::Ident(n) => n.clone(),
        _ => return,
    };

    let narrowed = match pred_name {
        "is_zero" => Some(Ty::app("atom", vec![TyArg::numeric("0")], "atom(0)")),
        "is_one" => Some(Ty::app("atom", vec![TyArg::numeric("1")], "atom(1)")),
        "unsigned" | "is_unsigned" => Some(Ty::named("nat")),
        _ => None,
    };

    if let Some(ty) = narrowed {
        env.narrow(Name::new(&arg_name), ty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::TyKind;

    #[test]
    fn empty_flow_env() {
        let env = FlowEnvironment::new();
        assert!(env.is_empty());
        assert_eq!(env.len(), 0);
    }

    #[test]
    fn narrow_and_lookup() {
        let mut env = FlowEnvironment::new();
        env.narrow(Name::new("x"), Ty::named("int"));
        assert!(!env.is_empty());
        assert_eq!(env.get(&Name::new("x")), Some(&Ty::named("int")));
        assert_eq!(env.get(&Name::new("y")), None);
    }

    #[test]
    fn narrowing_overrides() {
        let mut env = FlowEnvironment::new();
        env.narrow(Name::new("x"), Ty::named("int"));
        env.narrow(Name::new("x"), Ty::named("nat"));
        assert_eq!(env.get(&Name::new("x")), Some(&Ty::named("nat")));
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn merge_and_environments() {
        let mut env1 = FlowEnvironment::new();
        env1.narrow(Name::new("x"), Ty::named("int"));
        env1.narrow(Name::new("y"), Ty::named("bool"));

        let mut env2 = FlowEnvironment::new();
        env2.narrow(Name::new("x"), Ty::named("nat")); // overrides

        env1.merge_and(&env2);
        assert_eq!(env1.get(&Name::new("x")), Some(&Ty::named("nat")));
        assert_eq!(env1.get(&Name::new("y")), Some(&Ty::named("bool")));
    }

    #[test]
    fn merge_or_keeps_common() {
        let mut env1 = FlowEnvironment::new();
        env1.narrow(Name::new("x"), Ty::named("nat"));
        env1.narrow(Name::new("y"), Ty::named("bool"));

        let mut env2 = FlowEnvironment::new();
        env2.narrow(Name::new("x"), Ty::named("nat")); // same as env1
        env2.narrow(Name::new("z"), Ty::named("int")); // different var

        env1.merge_or(&env2);
        // Only "x" agrees in both
        assert_eq!(env1.get(&Name::new("x")), Some(&Ty::named("nat")));
        assert_eq!(env1.get(&Name::new("y")), None); // removed (not in env2)
    }

    #[test]
    fn narrow_from_guard_empty_for_missing_expr() {
        // Create a minimal body with a single Missing expression
        use hir_def::expr_store::ExpressionStoreBuilder;
        let mut builder = ExpressionStoreBuilder::new();
        let expr_id = builder.alloc_expr(Expr::Missing, parser::Span::new(0, 0));
        let (store, _source_map) = builder.finish();
        let body = Body::new(store, vec![], expr_id);

        let env = narrow_from_guard(&body, expr_id);
        assert!(env.is_empty());
    }

    #[test]
    fn narrow_from_guard_eq_literal() {
        // Build: `x == 5`
        use hir_def::expr_store::ExpressionStoreBuilder;
        let mut builder = ExpressionStoreBuilder::new();
        let x_id = builder.alloc_expr(Expr::Ident("x".to_string()), parser::Span::new(0, 1));
        let five_id = builder.alloc_expr(
            Expr::Literal(parser::Literal::Number("5".to_string())),
            parser::Span::new(5, 6),
        );
        let eq_id = builder.alloc_expr(
            Expr::BinaryOp {
                lhs: x_id,
                op: hir_def::hir::HirBinaryOp::from_str("=="),
                rhs: five_id,
            },
            parser::Span::new(0, 6),
        );
        let (store, _sm) = builder.finish();
        let body = Body::new(store, vec![], eq_id);

        let env = narrow_from_guard(&body, eq_id);
        assert!(!env.is_empty());
        let narrowed = env.get(&Name::new("x")).unwrap();
        // Should narrow to atom(5)
        match narrowed.kind() {
            TyKind::App { name, args, .. } => {
                assert_eq!(name, "atom");
                assert_eq!(args.len(), 1);
                match &args[0] {
                    TyArg::Nexp(n) => assert_eq!(n.to_string_repr(), "5"),
                    TyArg::Value(v) => assert_eq!(v, "5"),
                    other => panic!("expected numeric arg, got {:?}", other),
                }
            }
            other => panic!("expected App(atom, [5]), got {:?}", other),
        }
    }

    #[test]
    fn narrow_from_guard_ge_zero() {
        // Build: `x >= 0`
        use hir_def::expr_store::ExpressionStoreBuilder;
        let mut builder = ExpressionStoreBuilder::new();
        let x_id = builder.alloc_expr(Expr::Ident("x".to_string()), parser::Span::new(0, 1));
        let zero_id = builder.alloc_expr(
            Expr::Literal(parser::Literal::Number("0".to_string())),
            parser::Span::new(5, 6),
        );
        let cmp_id = builder.alloc_expr(
            Expr::BinaryOp {
                lhs: x_id,
                op: hir_def::hir::HirBinaryOp::from_str(">="),
                rhs: zero_id,
            },
            parser::Span::new(0, 6),
        );
        let (store, _sm) = builder.finish();
        let body = Body::new(store, vec![], cmp_id);

        let env = narrow_from_guard(&body, cmp_id);
        assert!(!env.is_empty());
        let narrowed = env.get(&Name::new("x")).unwrap();
        // Should narrow to nat
        assert_eq!(narrowed, &Ty::named("nat"));
    }

    #[test]
    fn narrow_from_guard_and_composition() {
        // Build: `x >= 0 & y == 1`
        use hir_def::expr_store::ExpressionStoreBuilder;
        let mut builder = ExpressionStoreBuilder::new();
        let x_id = builder.alloc_expr(Expr::Ident("x".to_string()), parser::Span::new(0, 1));
        let zero_id = builder.alloc_expr(
            Expr::Literal(parser::Literal::Number("0".to_string())),
            parser::Span::new(5, 6),
        );
        let cmp_id = builder.alloc_expr(
            Expr::BinaryOp {
                lhs: x_id,
                op: hir_def::hir::HirBinaryOp::from_str(">="),
                rhs: zero_id,
            },
            parser::Span::new(0, 6),
        );
        let y_id = builder.alloc_expr(Expr::Ident("y".to_string()), parser::Span::new(10, 11));
        let one_id = builder.alloc_expr(
            Expr::Literal(parser::Literal::Number("1".to_string())),
            parser::Span::new(15, 16),
        );
        let eq_id = builder.alloc_expr(
            Expr::BinaryOp {
                lhs: y_id,
                op: hir_def::hir::HirBinaryOp::from_str("=="),
                rhs: one_id,
            },
            parser::Span::new(10, 16),
        );
        let and_id = builder.alloc_expr(
            Expr::BinaryOp {
                lhs: cmp_id,
                op: hir_def::hir::HirBinaryOp::from_str("&"),
                rhs: eq_id,
            },
            parser::Span::new(0, 16),
        );
        let (store, _sm) = builder.finish();
        let body = Body::new(store, vec![], and_id);

        let env = narrow_from_guard(&body, and_id);
        assert_eq!(env.len(), 2);
        assert_eq!(env.get(&Name::new("x")), Some(&Ty::named("nat")));
        // y should be narrowed to atom(1)
        let y_ty = env.get(&Name::new("y")).unwrap();
        match y_ty.kind() {
            TyKind::App { name, .. } => assert_eq!(name, "atom"),
            other => panic!("expected App(atom, ...), got {:?}", other),
        }
    }
}
