//! Re-export type error types from hir-def (canonical location).
//! All existing `ide_diagnostics::type_error::*` paths continue to work.
pub use hir_def::type_error::{TypeError, VectorOrder};
