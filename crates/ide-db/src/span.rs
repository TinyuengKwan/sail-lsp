//! Span → internal-type adapters (framework-independent).
//!
//! LSP-specific converters (`to_lsp_position`, `from_lsp_position`,
//! `range_from_span`, `location_from_span`) have moved to the binary
//! crate boundary (`sail-lsp/src/lsp_ext.rs`). Only internal-type
//! helpers remain here.

use crate::ide_types::FileLocation;
use crate::line_index::TextRange;
use parser::Span;
use url::Url;

/// Convert a parser::Span to an internal TextRange.
pub fn text_range_from_span(span: Span) -> TextRange {
    base_db::span_to_text_range(&span)
}

/// Convert a parser::Span + URI to an internal FileLocation.
pub fn file_location_from_span(uri: &Url, span: Span) -> FileLocation {
    FileLocation { url: uri.clone(), range: text_range_from_span(span) }
}
