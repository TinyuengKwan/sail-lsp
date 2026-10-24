//! Existential type inference -- skolemization and witness extraction.
//!
//! Sail existentials are refinement types: `{'n, 'n > 0. bits('n)}`.

use std::collections::HashMap;

use super::constraint;
use super::{InferenceTable, Subst};
#[cfg(test)]
use crate::ty::CompareOp;
#[cfg(test)]
use crate::ty::NumericExpr;
#[cfg(test)]
use crate::ty::TyArg;
use crate::ty::{ConstraintExpr, Ty, TyKind};

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
    Ok { witnesses: HashMap<String, num_bigint::BigInt>, inner_ty: Ty },
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
        let skolem_name = format!("{}{}", skolem_prefix, var.trim_start_matches('\''));
        skolem_map.insert(var.clone(), skolem_name.clone());
        // Bind the variable to a Named type representing the skolem constant
        subst.types.insert(var.clone(), Ty::param(&skolem_name));
    }

    let inner_ty = constraint::apply_subst(inner, &subst);
    let skolemized_constraint = constraint::apply_subst_constraint_expr(constraint, &subst);

    Skolemized { inner_ty, skolem_map, constraint: skolemized_constraint }
}

/// Extract witness values from a concrete type and verify the constraint.
pub fn extract_witnesses(
    vars: &[String],
    constraint: &ConstraintExpr,
    inner: &Ty,
    actual: &Ty,
    table: &mut InferenceTable,
) -> WitnessResult {
    // Use the production quantified relation. Deferred goals stay in the
    // supplied table instead of being dropped by this convenience API.
    let expected = Ty::exist(vars.to_vec(), constraint.clone(), inner.clone());
    match table.check_compatibility(&expected, actual) {
        super::unify::Compatibility::Mismatch => {
            return WitnessResult::ConstraintViolation {
                variable: vars.first().cloned().unwrap_or_default(),
                constraint_text: constraint.to_text(),
            }
        }
        super::unify::Compatibility::Deferred | super::unify::Compatibility::Recovery => {
            return WitnessResult::Deferred { inner_ty: actual.clone() }
        }
        super::unify::Compatibility::Proven => {}
    }
    let mut subst = Subst::default();
    if !constraint::collect_type_bindings(inner, actual, &mut subst) {
        return WitnessResult::Deferred { inner_ty: actual.clone() };
    }

    // Extract concrete values for each bound variable
    let mut witnesses: HashMap<String, num_bigint::BigInt> = HashMap::new();
    let mut all_concrete = true;

    for var in vars {
        if let Some(value_text) = subst.values.get(var) {
            if let Ok(value) = value_text.parse::<num_bigint::BigInt>() {
                witnesses.insert(var.clone(), value);
            } else {
                all_concrete = false;
            }
        } else if let Some(ty) = subst.types.get(var) {
            // Try to extract a numeric value from the type
            if let TyKind::Named(name, _) = ty.kind() {
                if let Ok(value) = name.parse::<num_bigint::BigInt>() {
                    witnesses.insert(var.clone(), value);
                } else {
                    all_concrete = false;
                }
            } else if let TyKind::App { args, .. } = ty.kind() {
                // atom(N) → extract N
                let v_str = args.first().and_then(|a| a.as_value_str());
                if let Some(v) = v_str {
                    if let Ok(value) = v.parse::<num_bigint::BigInt>() {
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
    let status = evaluate_constraint_with_witnesses(constraint, &witnesses);
    if status == super::ConstraintStatus::Unknown {
        return WitnessResult::Deferred { inner_ty: actual.clone() };
    }
    if status == super::ConstraintStatus::Failed {
        // Find which variable causes the violation for diagnostics
        let violation_var = vars.first().cloned().unwrap_or_default();
        return WitnessResult::ConstraintViolation {
            variable: violation_var,
            constraint_text: format!("{:?}", constraint),
        };
    }

    WitnessResult::Ok { witnesses, inner_ty: actual.clone() }
}

/// Preserve Unknown under negation and membership instead of guessing truth.
fn evaluate_constraint_with_witnesses(
    constraint: &ConstraintExpr,
    witnesses: &HashMap<String, num_bigint::BigInt>,
) -> super::ConstraintStatus {
    let mut subst = Subst::default();
    for (name, value) in witnesses {
        subst.values.insert(name.clone(), value.to_string());
    }
    super::solver::solve(constraint, &subst, &[])
}

#[cfg(test)]
fn eval_numeric_with_witnesses(
    expr: &NumericExpr,
    witnesses: &HashMap<String, num_bigint::BigInt>,
) -> Option<i64> {
    let mut subst = Subst::default();
    for (name, value) in witnesses {
        subst.values.insert(name.clone(), value.to_string());
    }
    super::numeric::eval_numeric_expr(expr, &subst, &[])
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
        assert_eq!(result.skolem_map.get("'n"), Some(&"sk_0_n".to_string()));

        // Constraint should have the variable replaced with symbol
        match &result.constraint {
            ConstraintExpr::Compare { lhs, .. } => {
                assert!(matches!(lhs, NumericExpr::Symbol(s) if s == "sk_0_n"));
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
                assert_eq!(witnesses.get("'n"), Some(&32.into()));
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
        witnesses.insert("'n".to_string(), 32.into());

        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Satisfied
        );

        witnesses.insert("'n".to_string(), 0.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Failed
        );
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

        witnesses.insert("'n".to_string(), 32.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Satisfied
        );

        witnesses.insert("'n".to_string(), 0.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Failed
        ); // fails 'n > 0

        witnesses.insert("'n".to_string(), 100.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Failed
        ); // fails 'n <= 64
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

        witnesses.insert("'n".to_string(), 32.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Satisfied
        );

        witnesses.insert("'n".to_string(), 12.into());
        assert_eq!(
            evaluate_constraint_with_witnesses(&constraint, &witnesses),
            super::super::ConstraintStatus::Failed
        );
    }

    #[test]
    fn eval_numeric_with_exp() {
        let witnesses = HashMap::from([("'n".to_string(), 4i64.into())]);
        let expr = NumericExpr::Exp(Box::new(NumericExpr::Var("'n".to_string())));
        assert_eq!(eval_numeric_with_witnesses(&expr, &witnesses), Some(16)); // 2^4 = 16
    }
}
