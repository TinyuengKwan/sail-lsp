//! LSP protocol definitions and conversions.
//!
//! Contains: from_proto, to_proto, ext (extensions), capabilities.

pub(crate) mod capabilities;
pub(crate) mod ext;
// Shared with the corpus range-conversion checker.
pub mod from_proto;
pub(crate) mod semantic_tokens;
pub mod to_proto;
