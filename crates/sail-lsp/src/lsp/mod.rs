//! LSP protocol definitions and conversions.
//!
//! Contains: from_proto, to_proto, ext (extensions), capabilities.

pub(crate) mod capabilities;
pub(crate) mod ext;
pub(crate) mod from_proto;
pub(crate) mod semantic_tokens;
pub(crate) mod to_proto;
pub(crate) mod utils;
