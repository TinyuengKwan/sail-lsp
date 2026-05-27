//! Signature help — displays function signature at call site.

// Re-export from calls.rs (where the implementation lives)
pub use crate::calls::signature_help_ide as signature_help;
pub use crate::calls::{call_info_at_position, find_call_at_position, CallInfo};
