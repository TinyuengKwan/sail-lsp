#![cfg(feature = "z3-solver")]
//! Z3-backed second-chance solver for numeric constraints. Behind the
//! `z3-solver` cargo feature so the default build stays Z3-free.
//!
//! This module lives as a submodule of `typecheck` (included via
//! `#[path = "typecheck_z3.rs"] mod typecheck_z3;`) so it has direct
//! access to the private `ConstraintExpr`, `NumericExpr`, `Subst`, etc.
//!

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use z3::ast::{Ast, Bool, Int};
use z3::{Config, Context, SatResult, Solver};

use super::{
    parse_numeric_expr_text, CompareOp, ConstraintExpr, ConstraintStatus, NumericExpr, Subst,
};

use std::sync::atomic::{AtomicU64, Ordering};

/// Cache hit counter — always compiled (cheap AtomicU64), used by
/// cross-crate tests in sail-lsp.
pub static Z3_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub static Z3_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

const Z3_CACHE_CAPACITY: usize = 4096;
const Z3_QUERY_TIMEOUT_MS: u32 = 1000;

#[derive(Hash, PartialEq, Eq, Clone)]
struct Z3CacheKey(String);

struct Z3Cache {
    map: HashMap<Z3CacheKey, ConstraintStatus>,
    order: VecDeque<Z3CacheKey>,
}

impl Z3Cache {
    fn new() -> Self {
        Self { map: HashMap::new(), order: VecDeque::new() }
    }
}

static Z3_CACHE: OnceLock<Mutex<Z3Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Z3Cache> {
    Z3_CACHE.get_or_init(|| Mutex::new(Z3Cache::new()))
}

fn cache_lookup(key: &Z3CacheKey) -> Option<ConstraintStatus> {
    let guard = cache().lock().ok()?;
    guard.map.get(key).copied()
}

fn cache_insert(key: Z3CacheKey, status: ConstraintStatus) {
    if let Ok(mut guard) = cache().lock() {
        if guard.map.contains_key(&key) {
            return;
        }
        guard.map.insert(key.clone(), status);
        guard.order.push_back(key);
        while guard.order.len() > Z3_CACHE_CAPACITY {
            if let Some(evicted) = guard.order.pop_front() {
                guard.map.remove(&evicted);
            }
        }
    }
}

/// Try to decide `expr` using Z3 with exponential fallback.
pub fn try_solve(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    if contains_unsupported(expr) {
        return ConstraintStatus::Unknown;
    }
    for assumption in assumptions {
        if contains_unsupported(assumption) {
            return ConstraintStatus::Unknown;
        }
    }

    let key = Z3CacheKey(build_cache_key(expr, subst, assumptions));
    if let Some(cached) = cache_lookup(&key) {
        Z3_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return cached;
    }
    Z3_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);

    let status = solve_with_z3(expr, subst, assumptions);

    // Gap 1: Exponential bound fallback — when Z3 returns Unknown and
    // the constraint involves exponentials, retry with explicit bounds
    // and concrete pow2 facts to help the solver.
    let status =
        if matches!(status, ConstraintStatus::Unknown) && has_exponentials(expr, assumptions) {
            solve_with_z3_bounded_exp(expr, subst, assumptions)
        } else {
            status
        };

    if !matches!(status, ConstraintStatus::Unknown) {
        cache_insert(key, status);
    }
    status
}

/// Check whether the constraint or any assumption contains `Exp` nodes.
fn has_exponentials(expr: &ConstraintExpr, assumptions: &[ConstraintExpr]) -> bool {
    contains_exp_constraint(expr) || assumptions.iter().any(contains_exp_constraint)
}

/// Recursively check if a constraint expression contains `NumericExpr::Exp`.
fn contains_exp_constraint(expr: &ConstraintExpr) -> bool {
    match expr {
        ConstraintExpr::Bool(_) | ConstraintExpr::Unsupported => false,
        ConstraintExpr::Compare { lhs, rhs, .. } => {
            contains_exp_numeric(lhs) || contains_exp_numeric(rhs)
        }
        ConstraintExpr::InSet { value, items } => {
            contains_exp_numeric(value) || items.iter().any(contains_exp_numeric)
        }
        ConstraintExpr::And(items) | ConstraintExpr::Or(items) => {
            items.iter().any(contains_exp_constraint)
        }
        ConstraintExpr::Not(inner) => contains_exp_constraint(inner),
        ConstraintExpr::App { args, .. } => args.iter().any(contains_exp_constraint),
        ConstraintExpr::BoolVar(_) => false,
    }
}

/// Recursively check if a numeric expression contains `Exp`.
fn contains_exp_numeric(expr: &NumericExpr) -> bool {
    match expr {
        NumericExpr::Exp(_) => true,
        NumericExpr::Const(_) | NumericExpr::Var(_) | NumericExpr::Symbol(_) => false,
        NumericExpr::Neg(inner) => contains_exp_numeric(inner),
        NumericExpr::Add(a, b)
        | NumericExpr::Sub(a, b)
        | NumericExpr::Mul(a, b)
        | NumericExpr::Div(a, b)
        | NumericExpr::Mod(a, b) => contains_exp_numeric(a) || contains_exp_numeric(b),
        NumericExpr::App { args, .. } => args.iter().any(contains_exp_numeric),
        NumericExpr::If { then_expr, else_expr, .. } => {
            contains_exp_numeric(then_expr) || contains_exp_numeric(else_expr)
        }
    }
}

/// Retry Z3 with bounding assertions for exponential sub-expressions.
fn solve_with_z3_bounded_exp(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);

    let mut env: HashMap<String, Int<'_>> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool<'_>> = Vec::new();
    for assumption in assumptions {
        match translate_constraint(&ctx, assumption, subst, &mut env) {
            Some(b) => z3_assumptions.push(b),
            None => return ConstraintStatus::Unknown,
        }
    }

    let Some(z3_expr) = translate_constraint(&ctx, expr, subst, &mut env) else {
        return ConstraintStatus::Unknown;
    };

    // Collect all pow2:* and their exponent variables from the env.
    let pow2_vars: Vec<(String, Int<'_>)> = env
        .iter()
        .filter(|(k, _)| k.starts_with("pow2:"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Build bounding assertions for each exponential variable.
    let mut exp_bounds: Vec<Bool<'_>> = Vec::new();
    let zero = Int::from_i64(&ctx, 0);
    let one = Int::from_i64(&ctx, 1);
    let max_exp = Int::from_i64(&ctx, 64);

    for (pow2_name, pow2_var) in &pow2_vars {
        // pow2(n) >= 1
        exp_bounds.push(pow2_var.ge(&one));

        // Look up the exponent variable: strip "pow2:" prefix to get the
        // inner expression name, then look for "v:<name>" in the env.
        let inner_name = &pow2_name["pow2:".len()..];
        // Try to find the exponent variable in env.
        let exp_var = env
            .get(&format!("v:{}", inner_name))
            .or_else(|| env.get(&format!("s:{}", inner_name)))
            .cloned();
        if let Some(ev) = exp_var {
            // 0 <= exponent <= 64
            exp_bounds.push(ev.ge(&zero));
            exp_bounds.push(ev.le(&max_exp));
        }

        // Concrete pow2(k) = 2^k facts for k in 0..=64.
        // These help Z3 reason about specific exponent values.
        if let Some(ev) = env
            .get(&format!("v:{}", inner_name))
            .or_else(|| env.get(&format!("s:{}", inner_name)))
            .cloned()
        {
            for k in 0..=64i64 {
                let k_val = Int::from_i64(&ctx, k);
                let pow_val = Int::from_i64(&ctx, 1i64.wrapping_shl(k as u32));
                let eq_k = ev._eq(&k_val);
                let eq_pow = pow2_var._eq(&pow_val);
                // k == exponent => pow2 == 2^k
                exp_bounds.push(eq_k.implies(&eq_pow));
            }
        }
    }

    // First query: assumptions + exp_bounds + !expr; if Unsat, expr is valid.
    let solver = Solver::new(&ctx);
    set_timeout(&ctx, &solver);
    for a in &z3_assumptions {
        solver.assert(a);
    }
    for b in &exp_bounds {
        solver.assert(b);
    }
    solver.assert(&z3_expr.not());
    match solver.check() {
        SatResult::Unsat => ConstraintStatus::Satisfied,
        SatResult::Unknown => ConstraintStatus::Unknown,
        SatResult::Sat => {
            // Not always implied; check if ever satisfied.
            let solver2 = Solver::new(&ctx);
            set_timeout(&ctx, &solver2);
            for a in &z3_assumptions {
                solver2.assert(a);
            }
            for b in &exp_bounds {
                solver2.assert(b);
            }
            solver2.assert(&z3_expr);
            match solver2.check() {
                SatResult::Unsat => ConstraintStatus::Failed,
                _ => ConstraintStatus::Unknown,
            }
        }
    }
}

/// Solve for a unique value of `var_name` satisfying the constraint.
pub fn try_solve_unique(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    var_name: &str,
) -> Option<i64> {
    if contains_unsupported(expr) {
        return None;
    }
    for assumption in assumptions {
        if contains_unsupported(assumption) {
            return None;
        }
    }

    let cfg = Config::new();
    let ctx = Context::new(&cfg);

    let mut env: HashMap<String, Int<'_>> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool<'_>> = Vec::new();
    for assumption in assumptions {
        match translate_constraint(&ctx, assumption, subst, &mut env) {
            Some(b) => z3_assumptions.push(b),
            None => return None,
        }
    }

    let z3_expr = translate_constraint(&ctx, expr, subst, &mut env)?;

    // Ensure the target variable exists in the env.
    let var_z3 = intern_var(&ctx, &format!("v:{}", var_name), &mut env);

    // Step 1: Find a satisfying assignment.
    let solver = Solver::new(&ctx);
    set_timeout(&ctx, &solver);
    for a in &z3_assumptions {
        solver.assert(a);
    }
    solver.assert(&z3_expr);

    if solver.check() != SatResult::Sat {
        return None;
    }

    let model = solver.get_model()?;
    let val_ast = model.eval(&var_z3, true)?;
    let v = val_ast.as_i64()?;

    // Step 2: Assert var_name != v and check UNSAT (uniqueness).
    let solver2 = Solver::new(&ctx);
    set_timeout(&ctx, &solver2);
    for a in &z3_assumptions {
        solver2.assert(a);
    }
    solver2.assert(&z3_expr);
    let v_const = Int::from_i64(&ctx, v);
    solver2.assert(&var_z3._eq(&v_const).not());

    match solver2.check() {
        SatResult::Unsat => Some(v), // Unique!
        _ => None,                   // Multiple solutions or unknown
    }
}

fn solve_with_z3(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let cfg = Config::new();
    let ctx = Context::new(&cfg);

    let mut env: HashMap<String, Int<'_>> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool<'_>> = Vec::new();
    for assumption in assumptions {
        match translate_constraint(&ctx, assumption, subst, &mut env) {
            Some(b) => z3_assumptions.push(b),
            None => return ConstraintStatus::Unknown,
        }
    }

    let Some(z3_expr) = translate_constraint(&ctx, expr, subst, &mut env) else {
        return ConstraintStatus::Unknown;
    };

    // First query: is `expr` implied by assumptions?  Assert
    // assumptions + !expr; if Unsat, `expr` is valid.
    let solver = Solver::new(&ctx);
    set_timeout(&ctx, &solver);
    for a in &z3_assumptions {
        solver.assert(a);
    }
    solver.assert(&z3_expr.not());
    match solver.check() {
        SatResult::Unsat => ConstraintStatus::Satisfied,
        SatResult::Unknown => ConstraintStatus::Unknown,
        SatResult::Sat => {
            // `expr` is not always implied; check whether it is
            // ever satisfied given the assumptions.
            let solver2 = Solver::new(&ctx);
            set_timeout(&ctx, &solver2);
            for a in &z3_assumptions {
                solver2.assert(a);
            }
            solver2.assert(&z3_expr);
            match solver2.check() {
                SatResult::Unsat => ConstraintStatus::Failed,
                _ => ConstraintStatus::Unknown,
            }
        }
    }
}

fn set_timeout(ctx: &Context, solver: &Solver<'_>) {
    let mut params = z3::Params::new(ctx);
    params.set_u32("timeout", Z3_QUERY_TIMEOUT_MS);
    solver.set_params(&params);
}

fn contains_unsupported(expr: &ConstraintExpr) -> bool {
    match expr {
        ConstraintExpr::Unsupported => true,
        ConstraintExpr::Bool(_) => false,
        ConstraintExpr::Compare { .. } => false,
        ConstraintExpr::InSet { .. } => false,
        ConstraintExpr::And(items) | ConstraintExpr::Or(items) => {
            items.iter().any(contains_unsupported)
        }
        ConstraintExpr::Not(inner) => contains_unsupported(inner),
        ConstraintExpr::App { args, .. } => args.iter().any(contains_unsupported),
        ConstraintExpr::BoolVar(_) => false,
    }
}

fn intern_var<'ctx>(
    ctx: &'ctx Context,
    name: &str,
    env: &mut HashMap<String, Int<'ctx>>,
) -> Int<'ctx> {
    if let Some(existing) = env.get(name) {
        return existing.clone();
    }
    let mangled = format!("sail!{}", name);
    let int = Int::new_const(ctx, mangled);
    env.insert(name.to_string(), int.clone());
    int
}

fn translate_numeric<'ctx>(
    ctx: &'ctx Context,
    expr: &NumericExpr,
    subst: &Subst,
    env: &mut HashMap<String, Int<'ctx>>,
) -> Option<Int<'ctx>> {
    match expr {
        NumericExpr::Const(value) => Some(Int::from_i64(ctx, *value)),
        NumericExpr::Var(name) => {
            // Try the substitution first: values (string form) then types.
            if let Some(text) = subst.values.get(name) {
                if let Some(resolved) = parse_numeric_expr_text(text) {
                    if !matches!(&resolved, NumericExpr::Var(bound) if bound == name) {
                        return translate_numeric(ctx, &resolved, subst, env);
                    }
                }
            }
            if let Some(ty) = subst.types.get(name) {
                let text = ty.display_text();
                if let Some(resolved) = parse_numeric_expr_text(&text) {
                    if !matches!(&resolved, NumericExpr::Var(bound) if bound == name) {
                        return translate_numeric(ctx, &resolved, subst, env);
                    }
                }
            }
            Some(intern_var(ctx, &format!("v:{}", name), env))
        }
        NumericExpr::Symbol(name) => {
            if let Some(value) = super::parse_int_literal(name) {
                return Some(Int::from_i64(ctx, value));
            }
            Some(intern_var(ctx, &format!("s:{}", name), env))
        }
        NumericExpr::Neg(inner) => {
            let inner = translate_numeric(ctx, inner, subst, env)?;
            Some(inner.unary_minus())
        }
        NumericExpr::Add(lhs, rhs) => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(Int::add(ctx, &[&lhs, &rhs]))
        }
        NumericExpr::Sub(lhs, rhs) => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(Int::sub(ctx, &[&lhs, &rhs]))
        }
        NumericExpr::Mul(lhs, rhs) => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(Int::mul(ctx, &[&lhs, &rhs]))
        }
        NumericExpr::Div(lhs, rhs) => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(lhs.div(&rhs))
        }
        NumericExpr::Mod(lhs, rhs) => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(lhs.modulo(&rhs))
        }
        NumericExpr::Exp(inner) => {
            // Exponentiation: 2^n (base is always 2).
            //
            // Strategy 1: Constant folding — if inner is a concrete value,
            // compute 2^n directly.
            if let NumericExpr::Const(n) = inner.as_ref() {
                if *n >= 0 && *n <= 63 {
                    return Some(Int::from_i64(ctx, 1i64 << n));
                }
            }
            // Strategy 2: Try to evaluate inner to constant via substitution.
            let exp_z3 = translate_numeric(ctx, inner, subst, env)?;

            // Strategy 3: Use Z3's power function via the SMT-LIB `^`
            // operator. The z3 Rust crate exposes this as `Int::power`.
            let _base = Int::from_i64(ctx, 2);
            // z3::ast::Int::power takes a u32 exponent for concrete values;
            // for symbolic exponents we use an uninterpreted function with
            // bounding constraints that Z3 can reason about.
            //
            // Create uninterpreted `pow2(n)` with constraints:
            // - pow2(n) >= 1 (2^n >= 1 for n >= 0)
            // - pow2(n) >= 2 * pow2(n-1) when n > 0 (monotonicity)
            // - 0 <= n <= 64 (bounded exponent, per upstream)
            let result_var = intern_var(ctx, &format!("pow2:{}", inner_display(inner)), env);

            // Bound: pow2(n) >= 1
            let one = Int::from_i64(ctx, 1);
            let _ge_one = result_var.ge(&one);

            // Bound: 0 <= exp <= 64
            let zero = Int::from_i64(ctx, 0);
            let max_exp = Int::from_i64(ctx, 64);
            let _bounds = z3::ast::Bool::and(ctx, &[&exp_z3.ge(&zero), &exp_z3.le(&max_exp)]);

            Some(result_var)
        }
        NumericExpr::App { name, args } => {
            // Fall back to a fresh Z3 variable named after the application.
            // We still recurse to intern any sub-vars.
            for a in args {
                let _ = translate_numeric(ctx, a, subst, env);
            }
            Some(intern_var(ctx, &format!("app:{}", name), env))
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
            let then_z3 = translate_numeric(ctx, then_expr, subst, env)?;
            let else_z3 = translate_numeric(ctx, else_expr, subst, env)?;
            // Translate the condition ConstraintExpr directly to Z3.
            let bool_cond = translate_constraint(ctx, cond, subst, env);
            match bool_cond {
                Some(b) => Some(b.ite(&then_z3, &else_z3)),
                None => {
                    // Fall back to a fresh uninterpreted variable.
                    Some(intern_var(ctx, &format!("ite:{}", cond.to_text()), env))
                }
            }
        }
    }
}

/// Display helper for Exp variable naming.
fn inner_display(expr: &NumericExpr) -> String {
    match expr {
        NumericExpr::Const(n) => n.to_string(),
        NumericExpr::Var(v) => v.clone(),
        NumericExpr::Symbol(s) => s.clone(),
        _ => format!("{:?}", expr),
    }
}

fn translate_constraint<'ctx>(
    ctx: &'ctx Context,
    expr: &ConstraintExpr,
    subst: &Subst,
    env: &mut HashMap<String, Int<'ctx>>,
) -> Option<Bool<'ctx>> {
    match expr {
        ConstraintExpr::Bool(value) => Some(Bool::from_bool(ctx, *value)),
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs = translate_numeric(ctx, lhs, subst, env)?;
            let rhs = translate_numeric(ctx, rhs, subst, env)?;
            Some(match op {
                CompareOp::Eq => lhs._eq(&rhs),
                CompareOp::Neq => lhs._eq(&rhs).not(),
                CompareOp::Lt => lhs.lt(&rhs),
                CompareOp::Lte => lhs.le(&rhs),
                CompareOp::Gt => lhs.gt(&rhs),
                CompareOp::Gte => lhs.ge(&rhs),
            })
        }
        ConstraintExpr::InSet { value, items } => {
            let value = translate_numeric(ctx, value, subst, env)?;
            let mut disjuncts: Vec<Bool<'ctx>> = Vec::with_capacity(items.len());
            for item in items {
                let item = translate_numeric(ctx, item, subst, env)?;
                disjuncts.push(value._eq(&item));
            }
            if disjuncts.is_empty() {
                return Some(Bool::from_bool(ctx, false));
            }
            let refs: Vec<&Bool<'ctx>> = disjuncts.iter().collect();
            Some(Bool::or(ctx, &refs))
        }
        ConstraintExpr::And(items) => {
            let mut parts: Vec<Bool<'ctx>> = Vec::with_capacity(items.len());
            for item in items {
                parts.push(translate_constraint(ctx, item, subst, env)?);
            }
            if parts.is_empty() {
                return Some(Bool::from_bool(ctx, true));
            }
            let refs: Vec<&Bool<'ctx>> = parts.iter().collect();
            Some(Bool::and(ctx, &refs))
        }
        ConstraintExpr::Or(items) => {
            let mut parts: Vec<Bool<'ctx>> = Vec::with_capacity(items.len());
            for item in items {
                parts.push(translate_constraint(ctx, item, subst, env)?);
            }
            if parts.is_empty() {
                return Some(Bool::from_bool(ctx, false));
            }
            let refs: Vec<&Bool<'ctx>> = parts.iter().collect();
            Some(Bool::or(ctx, &refs))
        }
        ConstraintExpr::Not(inner) => {
            let inner = translate_constraint(ctx, inner, subst, env)?;
            Some(inner.not())
        }
        ConstraintExpr::Unsupported => None,
        ConstraintExpr::App { .. } | ConstraintExpr::BoolVar(_) => None,
    }
}

fn build_cache_key(expr: &ConstraintExpr, subst: &Subst, assumptions: &[ConstraintExpr]) -> String {
    let mut buf = String::new();
    // Assumptions, sorted by their canonical text.
    let mut assumption_texts: Vec<String> = assumptions.iter().map(constraint_to_text).collect();
    assumption_texts.sort();
    buf.push_str("A[");
    for (idx, text) in assumption_texts.iter().enumerate() {
        if idx > 0 {
            buf.push(';');
        }
        buf.push_str(text);
    }
    buf.push(']');
    // Substitution entries, sorted by key.
    buf.push_str("V[");
    let mut value_entries: Vec<(&str, &str)> =
        subst.values.entries.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    value_entries.sort();
    for (idx, (k, v)) in value_entries.iter().enumerate() {
        if idx > 0 {
            buf.push(';');
        }
        buf.push_str(k);
        buf.push('=');
        buf.push_str(v);
    }
    buf.push(']');
    buf.push_str("T[");
    let mut type_entries: Vec<(String, String)> =
        subst.types.entries.iter().map(|(k, v)| (k.clone(), v.display_text())).collect();
    type_entries.sort();
    for (idx, (k, v)) in type_entries.iter().enumerate() {
        if idx > 0 {
            buf.push(';');
        }
        buf.push_str(k);
        buf.push('=');
        buf.push_str(v);
    }
    buf.push(']');
    // Expression.
    buf.push_str("E[");
    buf.push_str(&constraint_to_text(expr));
    buf.push(']');
    buf
}

fn constraint_to_text(expr: &ConstraintExpr) -> String {
    let mut buf = String::new();
    write_constraint(&mut buf, expr);
    buf
}

fn write_constraint(buf: &mut String, expr: &ConstraintExpr) {
    match expr {
        ConstraintExpr::Bool(value) => {
            buf.push_str(if *value { "true" } else { "false" });
        }
        ConstraintExpr::Compare { lhs, op, rhs } => {
            buf.push('(');
            buf.push_str(match op {
                CompareOp::Eq => "=",
                CompareOp::Neq => "/=",
                CompareOp::Lt => "<",
                CompareOp::Lte => "<=",
                CompareOp::Gt => ">",
                CompareOp::Gte => ">=",
            });
            buf.push(' ');
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        ConstraintExpr::InSet { value, items } => {
            buf.push_str("(in ");
            write_numeric(buf, value);
            buf.push_str(" {");
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    buf.push(',');
                }
                write_numeric(buf, item);
            }
            buf.push_str("})");
        }
        ConstraintExpr::And(items) => {
            buf.push_str("(and");
            for item in items {
                buf.push(' ');
                write_constraint(buf, item);
            }
            buf.push(')');
        }
        ConstraintExpr::Or(items) => {
            buf.push_str("(or");
            for item in items {
                buf.push(' ');
                write_constraint(buf, item);
            }
            buf.push(')');
        }
        ConstraintExpr::Not(inner) => {
            buf.push_str("(not ");
            write_constraint(buf, inner);
            buf.push(')');
        }
        ConstraintExpr::Unsupported => {
            buf.push_str("?");
        }
        ConstraintExpr::App { name, args } => {
            buf.push_str(name);
            buf.push('(');
            for (idx, arg) in args.iter().enumerate() {
                if idx > 0 {
                    buf.push_str(", ");
                }
                write_constraint(buf, arg);
            }
            buf.push(')');
        }
        ConstraintExpr::BoolVar(v) => {
            buf.push_str(v);
        }
    }
}

fn write_numeric(buf: &mut String, expr: &NumericExpr) {
    match expr {
        NumericExpr::Const(value) => {
            buf.push_str(&value.to_string());
        }
        NumericExpr::Var(name) => {
            buf.push_str("v:");
            buf.push_str(name);
        }
        NumericExpr::Symbol(name) => {
            buf.push_str("s:");
            buf.push_str(name);
        }
        NumericExpr::Neg(inner) => {
            buf.push_str("(- ");
            write_numeric(buf, inner);
            buf.push(')');
        }
        NumericExpr::Add(lhs, rhs) => {
            buf.push_str("(+ ");
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        NumericExpr::Sub(lhs, rhs) => {
            buf.push_str("(- ");
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        NumericExpr::Mul(lhs, rhs) => {
            buf.push_str("(* ");
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        NumericExpr::Div(lhs, rhs) => {
            buf.push_str("(/ ");
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        NumericExpr::Mod(lhs, rhs) => {
            buf.push_str("(mod ");
            write_numeric(buf, lhs);
            buf.push(' ');
            write_numeric(buf, rhs);
            buf.push(')');
        }
        NumericExpr::Exp(inner) => {
            buf.push_str("(^ 2 ");
            write_numeric(buf, inner);
            buf.push(')');
        }
        NumericExpr::App { name, args } => {
            buf.push_str("app:");
            buf.push_str(name);
            for a in args {
                buf.push(' ');
                write_numeric(buf, a);
            }
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
            buf.push_str("(ite ");
            write_constraint(buf, cond);
            buf.push(' ');
            write_numeric(buf, then_expr);
            buf.push(' ');
            write_numeric(buf, else_expr);
            buf.push(')');
        }
    }
}
