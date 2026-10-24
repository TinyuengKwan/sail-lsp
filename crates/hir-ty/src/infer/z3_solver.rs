#![cfg(feature = "z3-solver")]
//! Z3-backed second-chance solver for numeric constraints. Behind the
//! `z3-solver` cargo feature (enabled by default; optional without default features).
//!
//! This module lives as a submodule of `typecheck` (included via
//! `#[path = "typecheck_z3.rs"] mod typecheck_z3;`) so it has direct
//! access to the private `ConstraintExpr`, `NumericExpr`, `Subst`, etc.
//!

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use z3::ast::{Ast, Bool, Int};
use z3::SatResult;

use super::{
    parse_numeric_expr_text, CompareOp, ConstraintExpr, ConstraintStatus, NumericExpr, Subst,
};

use std::sync::atomic::{AtomicU64, Ordering};

/// Cache hit counter — always compiled (cheap AtomicU64), used by
/// cross-crate tests in sail-lsp.
pub static Z3_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub static Z3_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

const Z3_CACHE_CAPACITY: usize = 4096;

#[derive(Hash, PartialEq, Eq, Clone)]
struct Z3CacheKey {
    goal: ConstraintExpr,
    assumptions: Vec<ConstraintExpr>,
    values: Vec<(String, String)>,
    types: Vec<(String, super::Ty)>,
}

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
/// Refute a universally quantified goal using an exact SMT translation.
/// Opaque functions and approximated powers cannot supply counterexamples.
pub(super) fn try_prove_quantified(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let _budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        Default::default(),
    );
    fn exact_numeric(e: &NumericExpr) -> bool {
        match e {
            NumericExpr::App { .. } | NumericExpr::Exp(_) => false,
            NumericExpr::Neg(x) => exact_numeric(x),
            NumericExpr::Add(a, b) | NumericExpr::Sub(a, b) | NumericExpr::Mul(a, b) => {
                exact_numeric(a) && exact_numeric(b)
            }
            NumericExpr::Div(a, b) | NumericExpr::Mod(a, b) => {
                exact_numeric(a) && matches!(b.as_ref(), NumericExpr::Const(n) if *n != 0)
            }
            NumericExpr::If { cond, then_expr, else_expr } => {
                exact(cond) && exact_numeric(then_expr) && exact_numeric(else_expr)
            }
            _ => true,
        }
    }
    fn exact(c: &ConstraintExpr) -> bool {
        match c {
            ConstraintExpr::Unsupported | ConstraintExpr::App { .. } => false,
            ConstraintExpr::Compare { lhs, rhs, .. } => exact_numeric(lhs) && exact_numeric(rhs),
            ConstraintExpr::InSet { value, items } => {
                exact_numeric(value) && items.iter().all(exact_numeric)
            }
            ConstraintExpr::And(xs) | ConstraintExpr::Or(xs) => xs.iter().all(exact),
            ConstraintExpr::Not(x) => exact(x),
            _ => true,
        }
    }
    let expr = super::constraint::apply_subst_constraint_expr(expr, subst);
    let assumptions: Vec<_> = assumptions
        .iter()
        .map(|a| super::constraint::apply_subst_constraint_expr(a, subst))
        .collect();
    if !exact(&expr) || !assumptions.iter().all(exact) {
        return ConstraintStatus::Unknown;
    }
    let mut env = HashMap::new();
    let solver = z3::Solver::new();
    for a in &assumptions {
        let Some(a) = translate_constraint(a, &Subst::default(), &mut env) else {
            return ConstraintStatus::Unknown;
        };
        solver.assert(a);
    }
    let Some(goal) = translate_constraint(&expr, &Subst::default(), &mut env) else {
        return ConstraintStatus::Unknown;
    };
    solver.assert(goal.not());
    match check_solver(&solver) {
        SatResult::Unsat => ConstraintStatus::Satisfied,
        SatResult::Sat => ConstraintStatus::Failed,
        SatResult::Unknown => ConstraintStatus::Unknown,
    }
}

pub fn try_solve(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let _budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        Default::default(),
    );
    if contains_unsupported(expr) || super::solver::has_erased_inputs(expr, subst, assumptions) {
        return ConstraintStatus::Unknown;
    }
    for assumption in assumptions {
        if contains_unsupported(assumption) {
            return ConstraintStatus::Unknown;
        }
    }

    let key = build_cache_key(expr, subst, assumptions);
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
        ConstraintExpr::App { args, .. } => args.iter().any(|a| match a {
            crate::ty::TyArg::Bool(c) => contains_exp_constraint(c),
            crate::ty::TyArg::Nexp(n) => contains_exp_numeric(n),
            _ => false,
        }),
        ConstraintExpr::BoolVar(_) => false,
    }
}

/// Recursively check if a numeric expression contains `Exp`.
fn contains_exp_numeric(expr: &NumericExpr) -> bool {
    match expr {
        NumericExpr::Exp(_) => true,
        NumericExpr::Const(_)
        | NumericExpr::BigConst(_)
        | NumericExpr::Var(_)
        | NumericExpr::Symbol(_) => false,
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
    let mut env: HashMap<String, Int> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool> = Vec::new();
    for assumption in assumptions {
        match translate_constraint(assumption, subst, &mut env) {
            Some(b) => z3_assumptions.push(b),
            None => return ConstraintStatus::Unknown,
        }
    }

    let Some(z3_expr) = translate_constraint(expr, subst, &mut env) else {
        return ConstraintStatus::Unknown;
    };

    // Collect all pow2:* and their exponent variables from the env.
    let pow2_vars: Vec<(String, Int)> = env
        .iter()
        .filter(|(k, _)| k.starts_with("pow2:"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Valid consequences only. Never assume a machine-word bound on Sail's
    // arbitrary-precision exponent, nor encode 2^63 as a negative integer.
    let mut exp_bounds: Vec<Bool> = Vec::new();
    let zero = Int::from_i64(0);
    let one = Int::from_i64(1);
    let two = Int::from_i64(2);
    for (pow2_name, pow2_var) in &pow2_vars {
        let Some(exponent) = env.get(&format!("pow2arg:{}", &pow2_name[5..])) else {
            continue;
        };
        exp_bounds.push(exponent.ge(&zero).implies(pow2_var.ge(&one)));
        let mut value = one.clone();
        for k in 0..=128i64 {
            exp_bounds.push(exponent.eq(Int::from_i64(k)).implies(pow2_var.eq(&value)));
            value = Int::mul(&[&value, &two]).simplify();
        }
    }

    // First query: assumptions + exp_bounds + !expr; if Unsat, expr is valid.
    let solver = z3::Solver::new();
    for a in &z3_assumptions {
        solver.assert(a);
    }
    for b in &exp_bounds {
        solver.assert(b);
    }
    solver.assert(z3_expr.not());
    match check_solver(&solver) {
        SatResult::Unsat => ConstraintStatus::Satisfied,
        SatResult::Unknown => ConstraintStatus::Unknown,
        SatResult::Sat => {
            // Not always implied; check if ever satisfied.
            let solver2 = z3::Solver::new();
            for a in &z3_assumptions {
                solver2.assert(a);
            }
            for b in &exp_bounds {
                solver2.assert(b);
            }
            solver2.assert(&z3_expr);
            match check_solver(&solver2) {
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
    let _budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        Default::default(),
    );
    if contains_unsupported(expr) {
        return None;
    }
    for assumption in assumptions {
        if contains_unsupported(assumption) {
            return None;
        }
    }

    let mut env: HashMap<String, Int> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool> = Vec::new();
    for assumption in assumptions {
        {
            let b = translate_constraint(assumption, subst, &mut env)?;
            z3_assumptions.push(b)
        }
    }

    let z3_expr = translate_constraint(expr, subst, &mut env)?;

    // Ensure the target variable exists in the env.
    let var_z3 = intern_var(&format!("v:{}", var_name), &mut env);

    // Step 1: Find a satisfying assignment.
    let solver = z3::Solver::new();
    for a in &z3_assumptions {
        solver.assert(a);
    }
    solver.assert(&z3_expr);

    if check_solver(&solver) != SatResult::Sat {
        return None;
    }

    let model = solver.get_model()?;
    let val_ast = model.eval(&var_z3, true)?;
    let v = val_ast.as_i64()?;

    // Step 2: Assert var_name != v and check UNSAT (uniqueness).
    let solver2 = z3::Solver::new();
    for a in &z3_assumptions {
        solver2.assert(a);
    }
    solver2.assert(&z3_expr);
    let v_const = Int::from_i64(v);
    solver2.assert(var_z3.eq(&v_const).not());

    match check_solver(&solver2) {
        SatResult::Unsat => Some(v), // Unique!
        _ => None,                   // Multiple solutions or unknown
    }
}

fn solve_with_z3(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let mut env: HashMap<String, Int> = HashMap::new();

    // Translate assumptions.
    let mut z3_assumptions: Vec<Bool> = Vec::new();
    for assumption in assumptions {
        match translate_constraint(assumption, subst, &mut env) {
            Some(b) => z3_assumptions.push(b),
            None => return ConstraintStatus::Unknown,
        }
    }

    let Some(z3_expr) = translate_constraint(expr, subst, &mut env) else {
        return ConstraintStatus::Unknown;
    };

    // First query: is `expr` implied by assumptions?  Assert
    // assumptions + !expr; if Unsat, `expr` is valid.
    let solver = z3::Solver::new();
    for a in &z3_assumptions {
        solver.assert(a);
    }
    solver.assert(z3_expr.not());
    match check_solver(&solver) {
        SatResult::Unsat => ConstraintStatus::Satisfied,
        SatResult::Unknown => ConstraintStatus::Unknown,
        SatResult::Sat => {
            // `expr` is not always implied; check whether it is
            // ever satisfied given the assumptions.
            let solver2 = z3::Solver::new();
            for a in &z3_assumptions {
                solver2.assert(a);
            }
            solver2.assert(&z3_expr);
            match check_solver(&solver2) {
                SatResult::Unsat => ConstraintStatus::Failed,
                _ => ConstraintStatus::Unknown,
            }
        }
    }
}

fn check_solver(solver: &z3::Solver) -> SatResult {
    let Some(permit) = crate::solver_budget::start_check() else {
        crate::solver_budget::SolverBudget::mark_budget_exhausted();
        crate::solver_budget::SolverBudget::mark_incomplete();
        return SatResult::Unknown;
    };
    // Do not use Z3's native timeout timer: libz3 4.8.12 can deadlock
    // concurrent contexts while recycling its timer workers. A scoped watchdog
    // borrows the documented cross-thread interrupt handle and is joined before
    // the context can be reused or destroyed. No detached computation survives.
    let mut params = z3::Params::new();
    params.set_u32("rlimit", permit.timeout_ms.saturating_mul(2000));
    solver.set_params(&params);
    let tokens = permit.cancellation_tokens();
    let interrupted = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let (done, receiver) = std::sync::mpsc::channel::<()>();
        let handle = solver.get_context().handle();
        let interrupted = &interrupted;
        let timeout = std::time::Duration::from_millis(permit.timeout_ms as u64);
        let deadline = std::time::Instant::now() + timeout;
        scope.spawn(move || loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            // Consume completion before interrupting: a delayed watchdog may
            // start after a quick check has already sent its result.
            match receiver.recv_timeout(remaining.min(std::time::Duration::from_millis(5))) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if std::time::Instant::now() >= deadline
                || tokens.iter().any(crate::CancellationToken::is_cancelled)
            {
                interrupted.store(true, Ordering::Relaxed);
                handle.interrupt();
                break;
            }
        });
        let result = solver.check();
        let _ = done.send(());
        result
    });
    if result == SatResult::Unknown || permit.cancelled() || interrupted.load(Ordering::Relaxed) {
        let reason = solver.get_reason_unknown().unwrap_or_default();
        if interrupted.load(Ordering::Relaxed)
            || reason.contains("resource")
            || reason.contains("timeout")
            || reason.contains("canceled")
        {
            crate::solver_budget::SolverBudget::mark_budget_exhausted();
        }
        crate::solver_budget::SolverBudget::mark_incomplete();
        SatResult::Unknown
    } else {
        result
    }
}

fn contains_unsupported(expr: &ConstraintExpr) -> bool {
    match expr {
        ConstraintExpr::Unsupported => true,
        ConstraintExpr::Bool(_) => false,
        ConstraintExpr::Compare { .. } | ConstraintExpr::InSet { .. } => {
            super::solver::has_erased_numeric(expr)
        }
        ConstraintExpr::And(items) | ConstraintExpr::Or(items) => {
            items.iter().any(contains_unsupported)
        }
        ConstraintExpr::Not(inner) => contains_unsupported(inner),
        ConstraintExpr::App { args, .. } => args.iter().any(|a| match a {
            crate::ty::TyArg::Bool(c) => contains_unsupported(c),
            crate::ty::TyArg::Type(_) | crate::ty::TyArg::Order(_) | crate::ty::TyArg::Value(_) => {
                true
            }
            _ => false,
        }),
        ConstraintExpr::BoolVar(_) => false,
    }
}

fn intern_var(name: &str, env: &mut HashMap<String, Int>) -> Int {
    if let Some(existing) = env.get(name) {
        return existing.clone();
    }
    let mangled = format!("sail!{}", name);
    let int = Int::new_const(mangled);
    env.insert(name.to_string(), int.clone());
    int
}

fn uninterpreted_numeric(name: &str, args: &[Int]) -> Int {
    let sort = z3::Sort::int();
    let domain = vec![&sort; args.len()];
    let function = z3::FuncDecl::new(format!("sail!{name}/{}", args.len()), &domain, &sort);
    let args: Vec<&dyn z3::ast::Ast> = args.iter().map(|a| a as &dyn z3::ast::Ast).collect();
    function.apply(&args).as_int().expect("integer function result")
}

fn translate_numeric(
    expr: &NumericExpr,
    subst: &Subst,
    env: &mut HashMap<String, Int>,
) -> Option<Int> {
    match expr {
        NumericExpr::Const(value) => Some(Int::from_i64(*value)),
        NumericExpr::BigConst(value) => value.to_string().parse::<Int>().ok(),
        NumericExpr::Var(name) => {
            let marker = format!("resolving:{name}");
            if env.contains_key(&marker) {
                return None;
            }
            let replacement =
                subst.values.get(name).and_then(|s| parse_numeric_expr_text(s)).or_else(|| {
                    subst.types.get(name).and_then(|ty| parse_numeric_expr_text(&ty.display_text()))
                });
            if let Some(replacement) = replacement {
                if !matches!(&replacement, NumericExpr::Var(n) if n == name) {
                    env.insert(marker.clone(), Int::from_i64(0));
                    let value = translate_numeric(&replacement, subst, env);
                    env.remove(&marker);
                    return value;
                }
            }
            Some(intern_var(&format!("v:{name}"), env))
        }
        NumericExpr::Symbol(name) => {
            if let Some(value) = super::parse_int_literal(name) {
                return Some(Int::from_i64(value));
            }
            Some(intern_var(&format!("s:{}", name), env))
        }
        NumericExpr::Neg(inner) => {
            let inner = translate_numeric(inner, subst, env)?;
            Some(inner.unary_minus())
        }
        NumericExpr::Add(lhs, rhs) => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(Int::add(&[&lhs, &rhs]))
        }
        NumericExpr::Sub(lhs, rhs) => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(Int::sub(&[&lhs, &rhs]))
        }
        NumericExpr::Mul(lhs, rhs) => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(Int::mul(&[&lhs, &rhs]))
        }
        NumericExpr::Div(lhs, rhs) => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(lhs.div(&rhs))
        }
        NumericExpr::Mod(lhs, rhs) => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(lhs.modulo(&rhs))
        }
        NumericExpr::Exp(inner) => {
            let exponent = translate_numeric(inner, subst, env)?;
            if let Some(n) = exponent.simplify().as_i64() {
                if (0..=1024).contains(&n) {
                    let mut value = Int::from_i64(1);
                    let two = Int::from_i64(2);
                    for _ in 0..n {
                        value = Int::mul(&[&value, &two]).simplify();
                    }
                    return Some(value);
                }
            }
            let value = uninterpreted_numeric("pow2", &[exponent.clone()]);
            // AST text is after substitution, unlike source text. Equal
            // arguments share a function application; distinct ones do not.
            let key = exponent.to_string();
            env.insert(format!("pow2:{key}"), value.clone());
            env.insert(format!("pow2arg:{key}"), exponent);
            Some(value)
        }
        NumericExpr::App { name, args } => {
            let args = args
                .iter()
                .map(|arg| translate_numeric(arg, subst, env))
                .collect::<Option<Vec<_>>>()?;
            match (name.as_str(), args.as_slice()) {
                ("div", [a, b]) => Some(a.div(b)),
                ("mod", [a, b]) => Some(a.modulo(b)),
                ("abs", [a]) => Some(a.ge(Int::from_i64(0)).ite(a, &a.unary_minus())),
                ("min", [a, b]) => Some(a.le(b).ite(a, b)),
                ("max", [a, b]) => Some(a.ge(b).ite(a, b)),
                _ => Some(uninterpreted_numeric(&format!("app:{name}"), &args)),
            }
        }
        NumericExpr::If { cond, then_expr, else_expr } => {
            let then_z3 = translate_numeric(then_expr, subst, env)?;
            let else_z3 = translate_numeric(else_expr, subst, env)?;
            // Translate the condition ConstraintExpr directly to Z3.
            let bool_cond = translate_constraint(cond, subst, env);
            match bool_cond {
                Some(b) => Some(b.ite(&then_z3, &else_z3)),
                None => {
                    // Fall back to a fresh uninterpreted variable.
                    Some(intern_var(&format!("ite:{}", cond.to_text()), env))
                }
            }
        }
    }
}

/// Display helper for Exp variable naming.
fn translate_constraint(
    expr: &ConstraintExpr,
    subst: &Subst,
    env: &mut HashMap<String, Int>,
) -> Option<Bool> {
    match expr {
        ConstraintExpr::Bool(value) => Some(Bool::from_bool(*value)),
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs = translate_numeric(lhs, subst, env)?;
            let rhs = translate_numeric(rhs, subst, env)?;
            Some(match op {
                CompareOp::Eq => lhs.eq(&rhs),
                CompareOp::Neq => lhs.eq(&rhs).not(),
                CompareOp::Lt => lhs.lt(&rhs),
                CompareOp::Lte => lhs.le(&rhs),
                CompareOp::Gt => lhs.gt(&rhs),
                CompareOp::Gte => lhs.ge(&rhs),
            })
        }
        ConstraintExpr::InSet { value, items } => {
            let value = translate_numeric(value, subst, env)?;
            let mut disjuncts: Vec<Bool> = Vec::with_capacity(items.len());
            for item in items {
                let item = translate_numeric(item, subst, env)?;
                disjuncts.push(value.eq(&item));
            }
            if disjuncts.is_empty() {
                return Some(Bool::from_bool(false));
            }
            let refs: Vec<&Bool> = disjuncts.iter().collect();
            Some(Bool::or(&refs))
        }
        ConstraintExpr::And(items) => {
            let mut parts: Vec<Bool> = Vec::with_capacity(items.len());
            for item in items {
                parts.push(translate_constraint(item, subst, env)?);
            }
            if parts.is_empty() {
                return Some(Bool::from_bool(true));
            }
            let refs: Vec<&Bool> = parts.iter().collect();
            Some(Bool::and(&refs))
        }
        ConstraintExpr::Or(items) => {
            let mut parts: Vec<Bool> = Vec::with_capacity(items.len());
            for item in items {
                parts.push(translate_constraint(item, subst, env)?);
            }
            if parts.is_empty() {
                return Some(Bool::from_bool(false));
            }
            let refs: Vec<&Bool> = parts.iter().collect();
            Some(Bool::or(&refs))
        }
        ConstraintExpr::Not(inner) => {
            let inner = translate_constraint(inner, subst, env)?;
            Some(inner.not())
        }
        ConstraintExpr::Unsupported => None,
        ConstraintExpr::BoolVar(name) => {
            let marker = format!("resolving-bool:{name}");
            if env.contains_key(&marker) {
                return None;
            }
            if let Some(text) = subst
                .values
                .get(name)
                .cloned()
                .or_else(|| subst.types.get(name).map(|t| t.display_text()))
            {
                let replacement = super::parse_constraint_text(&text);
                if replacement != *expr {
                    env.insert(marker.clone(), Int::from_i64(0));
                    let result = translate_constraint(&replacement, subst, env);
                    env.remove(&marker);
                    return result;
                }
            }
            Some(Bool::new_const(format!("sail!bool:{name}")))
        }
        ConstraintExpr::App { name, args } => {
            use crate::ty::TyArg;
            use z3::ast::{Ast, Dynamic};
            let values = args
                .iter()
                .map(|a| match a {
                    TyArg::Bool(c) => translate_constraint(c, subst, env).map(Dynamic::from),
                    TyArg::Nexp(n) => translate_numeric(n, subst, env).map(Dynamic::from),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let sorts: Vec<_> = values.iter().map(Ast::get_sort).collect();
            let signature = sorts.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
            let domain: Vec<_> = sorts.iter().collect();
            let fun = z3::FuncDecl::new(
                format!("sail!predicate:{name}({signature})"),
                &domain,
                &z3::Sort::bool(),
            );
            let args: Vec<&dyn Ast> = values.iter().map(|a| a as &dyn Ast).collect();
            fun.apply(&args).as_bool()
        }
    }
}

fn build_cache_key(
    expr: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Z3CacheKey {
    let mut values = subst.values.entries.to_vec();
    let mut types = subst.types.entries.to_vec();
    values.sort_by(|a, b| a.0.cmp(&b.0));
    types.sort_by(|a, b| a.0.cmp(&b.0));
    Z3CacheKey { goal: expr.clone(), assumptions: assumptions.to_vec(), values, types }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn a_running_solver_observes_request_cancellation() {
        use crate::solver_budget::{SolverBudget, SolverLimits};
        use std::time::{Duration, Instant};
        let token = crate::CancellationToken::new();
        let _budget = SolverBudget::enter(
            token.clone(),
            SolverLimits {
                total: Duration::from_secs(1),
                per_check: Duration::from_secs(1),
                checks: 1,
            },
        );
        let solver = z3::Solver::new();
        // An unsatisfiable pigeonhole instance keeps the native solver busy
        // long enough to exercise the interrupt handle, without large numerals.
        let cells: Vec<Vec<Bool>> = (0..24)
            .map(|p| (0..23).map(|h| Bool::new_const(format!("cancel_p{p}_h{h}"))).collect())
            .collect();
        for row in &cells {
            solver.assert(Bool::or(&row.iter().collect::<Vec<_>>()));
        }
        for h in 0..23 {
            for p in 0..24 {
                for q in p + 1..24 {
                    solver.assert(Bool::or(&[&cells[p][h].not(), &cells[q][h].not()]));
                }
            }
        }
        let started = Instant::now();
        let status = std::thread::scope(|scope| {
            scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                token.cancel();
            });
            check_solver(&solver)
        });
        assert_eq!(status, SatResult::Unknown);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancellation should interrupt native checking"
        );
        assert!(SolverBudget::incomplete_count() > 0);
    }
}
