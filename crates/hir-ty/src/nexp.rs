//! Compatibility names for the shared numeric/boolean AST.
//! Parsing, substitution, simplification and SMT now use the same representation.
pub use crate::ty::{ConstraintExpr as NConstraint, NumericExpr as Nexp};
