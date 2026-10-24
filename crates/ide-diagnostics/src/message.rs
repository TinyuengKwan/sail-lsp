//! Re-export message types from hir-def (canonical location).
//! All existing `ide_diagnostics::message::*` paths continue to work.
pub use hir::message::{Message, MessageSeverity};
