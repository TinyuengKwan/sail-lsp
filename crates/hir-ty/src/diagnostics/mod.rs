//! Body validation diagnostics — structural issues detected AFTER inference.
//! Separate from `InferenceDiagnostic` (type errors detected DURING inference).
//!
//! These diagnostics inspect the Body for patterns that are technically
//! valid but indicate likely bugs or style issues.
//!
//! Created as independent pass with immutable references.
//! Uses `ExprScopes` for unified binding enumeration.

/// Pattern exhaustiveness checking.
pub mod match_check;

/// Body validation diagnostics (unused variables, trailing return, unnecessary else).
pub mod expr;

/// Declaration-level convention checks (naming style).
pub mod decl_check;

pub use expr::BodyValidationDiagnostic;
