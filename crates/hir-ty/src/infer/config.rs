//! Checking global constraints after configuration instantiates abstract types.

use std::collections::HashMap;

use super::{
    parse_constraint_text, solver, CompareOp, ConstraintExpr, ConstraintStatus, NumericExpr, Subst,
};

/// Unknown configuration values stay symbolic. Only a proved contradiction
/// is an error; an editor may be looking at an incomplete configuration.
pub fn configured_constraint_status(
    constraint: &str,
    values: &HashMap<String, String>,
) -> ConstraintStatus {
    let mut assumptions = Vec::new();
    for (name, value) in values {
        // Configured numeric types have integer values. Do not interpret a
        // JSON string or object as a type-level expression.
        if let Some(value) = NumericExpr::integer_literal(value) {
            assumptions.push(ConstraintExpr::Compare {
                lhs: NumericExpr::Symbol(name.clone()),
                op: CompareOp::Eq,
                rhs: value,
            });
        }
    }
    solver::solve(&parse_constraint_text(constraint), &Subst::default(), &assumptions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiated_constraints_are_checked_without_assuming_missing_values() {
        let values = HashMap::from([("xlen".into(), "64".into()), ("maskmax".into(), "64".into())]);
        assert_eq!(
            configured_constraint_status("0 < maskmax & maskmax < xlen", &values),
            ConstraintStatus::Failed
        );
        assert_eq!(
            configured_constraint_status("xlen in {32, 64}", &values),
            ConstraintStatus::Satisfied
        );
        assert_eq!(
            configured_constraint_status("unknown < xlen", &values),
            ConstraintStatus::Unknown
        );
    }
}
