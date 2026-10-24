//! LSP protocol definitions and conversions.
//!
//! Contains: from_proto, to_proto, ext (extensions), capabilities.

pub(crate) mod capabilities;
pub(crate) mod ext;
// Public so `examples/protocol_check.rs` can measure through them. They are
// pure conversions with no invariant to protect, and the probe has to enter
// where the product does — the rule `corpus_check` states in its own header.
pub mod from_proto;
pub(crate) mod semantic_tokens;
pub mod to_proto;
pub(crate) mod utils;
