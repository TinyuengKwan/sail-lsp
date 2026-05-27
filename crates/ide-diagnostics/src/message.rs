//! Re-export message types from hir-def (canonical location).
//! All existing `ide_diagnostics::message::*` paths continue to work.
pub use hir_def::message::{Message, MessageSeverity};
