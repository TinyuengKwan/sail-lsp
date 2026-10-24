//! Re-export type error types from hir-def (canonical location).
//! All existing `ide_diagnostics::type_error::*` paths continue to work.
pub use hir::type_error::{TypeError, VectorOrder};
