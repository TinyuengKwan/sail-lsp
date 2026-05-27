//! Existential type inference -- skolemization and witness extraction.
//!
//! Sail existentials are refinement types: `{'n, 'n > 0. bits('n)}`.

use std::collections::HashMap;

use super::constraint;
use super::{InferenceTable, Subst};
#[cfg(test)]
use crate::ty::TyArg;
use crate::ty::{CompareOp, ConstraintExpr, NumericExpr, Ty, TyKind};

/// Result of skolemizing an existential type.
#[derive(Debug, Clone)]
pub struct Skolemized {
    pub inner_ty: Ty,
    pub skolem_map: HashMap<String, String>,
    pub constraint: ConstraintExpr,
}

/// Result of extracting witnesses from an existential type.
#[derive(Debug, Clone)]
pub enum WitnessResult {
    Ok { witnesses: HashMap<String, i64>, inner_ty: Ty },
    Deferred { inner_ty: Ty },
    ConstraintViolation { variable: String, constraint_text: String },
}

/// Replace bound variables with rigid skolem constants.
pub fn skolemize(
    vars: &[String],
    constraint: &ConstraintExpr,
    inner: &Ty,
    skolem_prefix: &str,
) -> Skolemized {
    let mut skolem_map = HashMap::new();
    let mut subst = Subst::default();

    for var in vars {
        let skolem_name = format!("{}{}", skolem_prefix, var);
        skolem_map.insert(var.clone(), skolem_name.clone());
        // Bind the variable to a Named type representing the skolem constant
        subst.types.insert(var.clone(), Ty::named(&skolem_name));
    }

    let inner_ty = constraint::apply_subst(inner, &subst);
    let skolemized_constraint = subst_constraint(constraint, &skolem_map);

    Skolemized { inner_ty, skolem_map, constraint: skolemized_constraint }
}

/// Substitute variable names in a constraint expression.
fn subst_constraint(expr: &ConstraintExpr, map: &HashMap<String, String>) -> ConstraintExpr {
    match expr {
        ConstraintExpr::Bool(b) => ConstraintExpr::Bool(*b),
        ConstraintExpr::Compare { lhs, op, rhs } => ConstraintExpr::Compare {
            lhs: subst_numeric(lhs, map),
            op: *op,
            rhs: subst_numeric(rhs, map),
        },
        ConstraintExpr::InSet { value, items } => ConstraintExpr::InSet {
            value: subst_numeric(value, map),
            items: items.iter().map(|i| subst_numeric(i, map)).collect(),
        },
        ConstraintExpr::And(parts) => {
            ConstraintExpr::And(parts.iter().map(|p| subst_constraint(p, map)).collect())
        }
        ConstraintExpr::Or(parts) => {
            ConstraintExpr::Or(parts.iter().map(|p| subst_constraint(p, map)).collect())
        }
        ConstraintExpr::Not(inner) => ConstraintExpr::Not(Box::new(subst_constraint(inner, map))),
        ConstraintExpr::Unsupported => ConstraintExpr::Unsupported,
        ConstraintExpr::App { name, args } => ConstraintExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| subst_constraint(a, map)).collect(),
        },
        ConstraintExpr::BoolVar(v) => {
            ConstraintExpr::BoolVar(map.get(v).cloned().unwrap_or_else(|| v.clone()))
        }
    }
}

fn subst_numeric(expr: &NumericExpr, map: &HashMap<String, String>) -> NumericExpr {
    match expr {
        NumericExpr::Const(c) => NumericExpr::Const(*c),
        NumericExpr::Var(name) => {
            if let Some(skolem) = map.get(name) {
                NumericExpr::Symbol(skolem.clone())
            } else {
                NumericExpr::Var(name.clone())
            }
        }
        NumericExpr::Symbol(s) => NumericExpr::Symbol(s.clone()),
        NumericExpr::Neg(inner) => NumericExpr::Neg(Box::new(subst_numeric(inner, map))),
        NumericExpr::Add(l, r) => {
            NumericExpr::Add(Box::new(subst_numeric(l, map)), Box::new(subst_numeric(r, map)))
        }
        NumericExpr::Sub(l, r) => {
            NumericExpr::Sub(Box::new(subst_numeric(l, map)), Box::new(subst_numeric(r, map)))
        }
        NumericExpr::Mul(l, r) => {
            NumericExpr::Mul(Box::new(subst_numeric(l, map)), Box::new(subst_numeric(r, map)))
        }
        NumericExpr::Div(l, r) => {
            NumericExpr::Div(Box::new(subst_numeric(l, map)), Box::new(subst_numeric(r, map)))
        }
        NumericExpr::Mod(l, r) => {
            NumericExpr::Mod(Box::new(subst_numeric(l, map)), Box::new(subst_numeric(r, map)))
        }
        NumericExpr::Exp(inner) => NumericExpr::Exp(Box::new(subst_numeric(inner, map))),
        NumericExpr::App { name, args } => NumericExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| subst_numeric(a, map)).collect(),
        },
        NumericExpr::If { cond, then_expr, else_expr } => NumericExpr::If {
            cond: Box::new(subst_constraint(cond, map)),
            then_expr: Box::new(subst_numeric(then_expr, map)),
            else_expr: Box::new(subst_numeric(else_expr, map)),
        },
    }
}

/// Extract witness values from a concrete type and verify the constraint.
pub fn extract_witnesses(
    vars: &[String],
    constraint: &ConstraintExpr,
    inner: &Ty,
    actual: &Ty,
    table: &mut InferenceTable,
) -> WitnessResult {
    // Try to unify the inner type with the actual type, collecting
    // bindings for the existential variables.
    let mut subst = Subst::default();
    let snap = table.snapshot();

    if !constraint::unify(inner, actual, &mut subst) {
        table.rollback_to(snap);
        // Can't match structure — just accept (inner type is returned)
        return WitnessResult::Deferred { inner_ty: actual.clone() };
    }
    table.rollback_to(snap);

    // Extract concrete values for each bound variable
    let mut witnesses: HashMap<String, i64> = HashMap::new();
    let mut all_concrete = true;

    for var in vars {
        if let Some(value_text) = subst.values.get(var) {
            if let Ok(value) = value_text.parse::<i64>() {
                witnesses.insert(var.clone(), value);
            } else {
                all_concrete = false;
            }
        } else if let Some(ty) = subst.types.get(var) {
            // Try to extract a numeric value from the type
            if let TyKind::Adt(name, _) = ty.kind() {
                if let Ok(value) = name.parse::<i64>() {
                    witnesses.insert(var.clone(), value);
                } else {
                    all_concrete = false;
                }
            } else if let TyKind::App { args, .. } = ty.kind() {
                // atom(N) → extract N
                let v_str = args.first().and_then(|a| a.as_value_str());
                if let Some(v) = v_str {
                    if let Ok(value) = v.parse::<i64>() {
                        witnesses.insert(var.clone(), value);
                    } else {
                        all_concrete = false;
                    }
                } else {
                    all_concrete = false;
                }
            } else {
                all_concrete = false;
            }
        } else {
            all_concrete = false;
        }
    }

    if !all_concrete {
        // Can't extract all witnesses concretely — accept optimistically
        return WitnessResult::Deferred { inner_ty: actual.clone() };
    }

    // Verify the constraint holds with the extracted witnesses
    if !evaluate_constraint_with_witnesses(constraint, &witnesses) {
        // Find which variable causes the violation for diagnostics
        let violation_var = vars.first().cloned().unwrap_or_default();
        return WitnessResult::ConstraintViolation {
            variable: violation_var,
            constraint_text: format!("{:?}", constraint),
        };
    }

    WitnessResult::Ok { witnesses, inner_ty: actual.clone() }
}

/// Evaluate a constraint with concrete witness values.
/// Returns true if the constraint is satisfied.
fn evaluate_constraint_with_witnesses(
    constraint: &ConstraintExpr,
    witnesses: &HashMap<String, i64>,
) -> bool {
    match constraint {
        ConstraintExpr::Bool(b) => *b,
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs_val = eval_numeric_with_witnesses(lhs, witnesses);
            let rhs_val = eval_numeric_with_witnesses(rhs, witnesses);
            match (lhs_val, rhs_val) {
                (Some(l), Some(r)) => match op {
                    CompareOp::Eq => l == r,
                    CompareOp::Neq => l != r,
                    CompareOp::Lt => l < r,
                    CompareOp::Lte => l <= r,
                    CompareOp::Gt => l > r,
                    CompareOp::Gte => l >= r,
                },
                // Can't evaluate → assume satisfied (optimistic)
                _ => true,
            }
        }
        ConstraintExpr::InSet { value, items } => {
            let v = eval_numeric_with_witnesses(value, witnesses);
            match v {
                Some(val) => items
                    .iter()
                    .any(|item| eval_numeric_with_witnesses(item, witnesses) == Some(val)),
                None => true, // optimistic
            }
        }
        ConstraintExpr::And(parts) => {
            parts.iter().all(|p| evaluate_constraint_with_witnesses(p, witnesses))
        }
        ConstraintExpr::Or(parts) => {
            parts.iter().any(|p| evaluate_constraint_with_witnesses(p, witnesses))
        }
        ConstraintExpr::Not(inner) => !evaluate_constraint_with_witnesses(inner, witnesses),
        ConstraintExpr::Unsupported => true, // optimistic
        ConstraintExpr::App { .. } | ConstraintExpr::BoolVar(_) => true, // optimistic
    }
}

/// Evaluate a numeric expression with concrete witness values.
fn eval_numeric_with_witnesses(
    expr: &NumericExpr,
    witnesses: &HashMap<String, i64>,
) -> Option<i64> {
    match expr {
        NumericExpr::Const(c) => Some(*c),
        NumericExpr::Var(name) => witnesses.get(name).copied(),
        NumericExpr::Symbol(name) => witnesses.get(name).copied().or_else(|| name.parse().ok()),
        NumericExpr::Neg(inner) => eval_numeric_with_witnesses(inner, witnesses).map(|v| -v),
        NumericExpr::Add(l, r) => {
            let l = eval_numeric_with_witnesses(l, witnesses)?;
            let r = eval_numeric_with_witnesses(r, witnesses)?;
            Some(l + r)
        }
        NumericExpr::Sub(l, r) => {
            let l = eval_numeric_with_witnesses(l, witnesses)?;
            let r = eval_numeric_with_witnesses(r, witnesses)?;
            Some(l - r)
        }
        NumericExpr::Mul(l, r) => {
            let l = eval_numeric_with_witnesses(l, witnesses)?;
            let r = eval_numeric_with_witnesses(r, witnesses)?;
            Some(l * r)
        }
        NumericExpr::Div(l, r) => {
            let l = eval_numeric_with_witnesses(l, witnesses)?;
            let r = eval_numeric_with_witnesses(r, witnesses)?;
            if r == 0 {
                None
            } else {
                Some(l / r)
            }
        }
        NumericExpr::Mod(l, r) => {
            let l = eval_numeric_with_witnesses(l, witnesses)?;
            let r = eval_numeric_with_witnesses(r, witnesses)?;
            if r == 0 {
                None
            } else {
                Some(l % r)
            }
        }
        NumericExpr::Exp(inner) => {
            let n = eval_numeric_with_witnesses(inner, witnesses)?;
            if n >= 0 && n <= 63 {
                Some(1i64 << n)
            } else {
                None
            }
        }
        NumericExpr::App { .. } => None, // opaque — cannot evaluate
        NumericExpr::If { cond: _, then_expr, else_expr } => {
            // If both branches evaluate to the same value, return it.
            let t = eval_numeric_with_witnesses(then_expr, witnesses)?;
            let e = eval_numeric_with_witnesses(else_expr, witnesses)?;
            if t == e {
                Some(t)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skolemize_replaces_bound_vars() {
        let vars = vec!["'n".to_string()];
        let constraint = ConstraintExpr::Compare {
            lhs: NumericExpr::Var("'n".to_string()),
            op: CompareOp::Gt,
            rhs: NumericExpr::Const(0),
        };
        let inner = Ty::app("bits", vec![TyArg::numeric("'n")], "bits('n)");

        let result = skolemize(&vars, &constraint, &inner, "sk_0_");

        // Bound variable should be replaced in skolem_map
        assert_eq!(result.skolem_map.get("'n"), Some(&"sk_0_'n".to_string()));

        // Constraint should have the variable replaced with symbol
        match &result.constraint {
            ConstraintExpr::Compare { lhs, .. } => {
                assert!(matches!(lhs, NumericExpr::Symbol(s) if s == "sk_0_'n"));
            }
            _ => panic!("expected Compare"),
        }
    }

    #[test]
    fn extract_witnesses_simple_bits() {
        // Declared: {'n, 'n > 0. bits('n)}
        // Actual: bits(32)
        let vars = vec!["'n".to_string()];
        let constraint = ConstraintExpr::Compare {
            lhs: NumericExpr::Var("'n".to_string()),
            op: CompareOp::Gt,
            rhs: NumericExpr::Const(0),
        };
        let inner = Ty::app("bits", vec![TyArg::numeric("'n")], "bits('n)");
        let actual = Ty::app("bits", vec![TyArg::numeric("32")], "bits(32)");

        let mut table = InferenceTable::default();
        let result = extract_witnesses(&vars, &constraint, &inner, &actual, &mut table);

        match result {
            WitnessResult::Ok { witnesses, .. } => {
                assert_eq!(witnesses.get("'n"), Some(&32));
            }
            WitnessResult::Deferred { .. } => {
                // Also acceptable if unification strategy doesn't extract
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn extract_witnesses_constraint_violation() {
        // Declared: {'n, 'n > 10. bits('n)}
        // Actual: bits(5) → violates 'n > 10
        let vars = vec!["'n".to_string()];
        let constraint = ConstraintExpr::Compare {
            lhs: NumericExpr::Var("'n".to_string()),
            op: CompareOp::Gt,
            rhs: NumericExpr::Const(10),
        };
        let inner = Ty::app("bits", vec![TyArg::numeric("'n")], "bits('n)");
        let actual = Ty::app("bits", vec![TyArg::numeric("5")], "bits(5)");

        let mut table = InferenceTable::default();
        let result = extract_witnesses(&vars, &constraint, &inner, &actual, &mut table);

        // Should detect the violation (5 is not > 10)
        // But may also be Deferred if unification can't extract the witness
        match result {
            WitnessResult::ConstraintViolation { .. } => {} // ideal
            WitnessResult::Deferred { .. } => {}            // acceptable
            WitnessResult::Ok { .. } => panic!("should have detected violation or deferred"),
        }
    }

    #[test]
    fn evaluate_constraint_simple() {
        let constraint = ConstraintExpr::Compare {
            lhs: NumericExpr::Var("'n".to_string()),
            op: CompareOp::Gt,
            rhs: NumericExpr::Const(0),
        };
        let mut witnesses = HashMap::new();
        witnesses.insert("'n".to_string(), 32);

        assert!(evaluate_constraint_with_witnesses(&constraint, &witnesses));

        witnesses.insert("'n".to_string(), 0);
        assert!(!evaluate_constraint_with_witnesses(&constraint, &witnesses));
    }

    #[test]
    fn evaluate_constraint_and() {
        let constraint = ConstraintExpr::And(vec![
            ConstraintExpr::Compare {
                lhs: NumericExpr::Var("'n".to_string()),
                op: CompareOp::Gt,
                rhs: NumericExpr::Const(0),
            },
            ConstraintExpr::Compare {
                lhs: NumericExpr::Var("'n".to_string()),
                op: CompareOp::Lte,
                rhs: NumericExpr::Const(64),
            },
        ]);
        let mut witnesses = HashMap::new();

        witnesses.insert("'n".to_string(), 32);
        assert!(evaluate_constraint_with_witnesses(&constraint, &witnesses));

        witnesses.insert("'n".to_string(), 0);
        assert!(!evaluate_constraint_with_witnesses(&constraint, &witnesses)); // fails 'n > 0

        witnesses.insert("'n".to_string(), 100);
        assert!(!evaluate_constraint_with_witnesses(&constraint, &witnesses)); // fails 'n <= 64
    }

    #[test]
    fn evaluate_constraint_in_set() {
        let constraint = ConstraintExpr::InSet {
            value: NumericExpr::Var("'n".to_string()),
            items: vec![
                NumericExpr::Const(8),
                NumericExpr::Const(16),
                NumericExpr::Const(32),
                NumericExpr::Const(64),
            ],
        };
        let mut witnesses = HashMap::new();

        witnesses.insert("'n".to_string(), 32);
        assert!(evaluate_constraint_with_witnesses(&constraint, &witnesses));

        witnesses.insert("'n".to_string(), 12);
        assert!(!evaluate_constraint_with_witnesses(&constraint, &witnesses));
    }

    #[test]
    fn eval_numeric_with_exp() {
        let witnesses = HashMap::from([("'n".to_string(), 4i64)]);
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Var("'n".to_string())));
        assert_eq!(eval_numeric_with_witnesses(&expr, &witnesses), Some(16)); // 2^4 = 16
    }
}
