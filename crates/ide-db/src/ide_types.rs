//! Framework-independent IDE domain types.
//!
//! Uses byte-offset ranges, not line/col positions. Conversion to LSP wire
//! types happens only in the binary crate via `to_proto`/`from_proto`.

use crate::line_index::TextRange;

/// Result type for IDE queries that can be cancelled.
pub type Cancellable<T> = Result<T, CancelledError>;

/// Error type for cancelled queries.
#[derive(Debug)]
pub struct CancelledError;

impl std::fmt::Display for CancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "query cancelled")
    }
}

impl std::error::Error for CancelledError {}

/// TextEdit canonical definition moved to text_edit.rs.
/// Re-exported here as IdeTextEdit for backward compatibility.
pub use crate::text_edit::TextEdit as IdeTextEdit;

/// A location in a file: URL + byte-offset range.
///
/// Replaces `lsp_types::Location`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLocation {
    pub url: url::Url,
    pub range: TextRange,
}

/// `NavigationTarget` used to live here. It is `ide::navigation_target`
/// now, beside the features that build it — rust-analyzer's placement, and
/// this crate only ever mentioned it in a test of its own.
/// SymbolKind canonical definition moved to defs.rs.
pub use crate::defs::SymbolKind;

/// Signature help information.
#[derive(Clone, Debug)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInfo>,
    pub active_signature: Option<usize>,
    pub active_parameter: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct SignatureInfo {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<ParameterInfo>,
}

#[derive(Clone, Debug)]
pub struct ParameterInfo {
    pub label: String,
}

/// A code lens at a byte-offset range.
#[derive(Clone, Debug)]
pub struct Annotation {
    pub range: TextRange,
    pub title: String,
    pub command: Option<String>,
    pub data: Option<serde_json::Value>,
}

/// A selection range (byte offsets, recursive).
#[derive(Clone, Debug)]
pub struct SelectionRange {
    pub range: TextRange,
    pub parent: Option<Box<SelectionRange>>,
}

/// A linked editing range set.
#[derive(Clone, Debug)]
pub struct LinkedEditingRanges {
    pub ranges: Vec<TextRange>,
    pub word_pattern: Option<String>,
}

/// A document link (byte offsets).
#[derive(Clone, Debug)]
pub struct DocumentLink {
    pub range: TextRange,
    pub target: Option<String>,
    pub tooltip: Option<String>,
}

/// Formatting options (framework-independent).
///
/// Replaces `lsp_types::FormattingOptions`.
#[derive(Clone, Debug)]
pub struct FormatOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub trim_final_newlines: Option<bool>,
    /// Maximum line width before wrapping. `None` disables wrapping.
    pub max_line_width: Option<u32>,
}

impl FormatOptions {
    pub fn tab_spaces(&self) -> usize {
        self.tab_size.max(1) as usize
    }

    pub fn max_width(&self) -> usize {
        self.max_line_width.map(|w| w as usize).unwrap_or(100)
    }

    pub fn hard_tabs(&self) -> bool {
        !self.insert_spaces
    }
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            trim_trailing_whitespace: None,
            insert_final_newline: None,
            trim_final_newlines: None,
            max_line_width: Some(100),
        }
    }
}

/// A call hierarchy item (framework-independent).
///
/// Replaces `lsp_types::CallHierarchyItem`.
#[derive(Clone, Debug)]
pub struct CallItem {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    pub url: url::Url,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub data: Option<serde_json::Value>,
}

/// A type hierarchy item (framework-independent).
///
/// Replaces `lsp_types::TypeHierarchyItem`.
#[derive(Clone, Debug)]
pub struct TypeHierarchyItem {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    pub url: url::Url,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub data: Option<serde_json::Value>,
}

/// A call edge between caller and callee.
#[derive(Clone, Debug)]
pub struct CallEdge {
    pub caller: String,
    pub caller_uri: url::Url,
    pub callee: String,
    pub call_range: TextRange,
}

/// An incoming call item with aggregated call sites.
///
/// caller function into a single item with multiple ranges.
#[derive(Clone, Debug)]
pub struct IncomingCallItem {
    /// Name of the calling function.
    pub caller: String,
    /// File containing the caller.
    pub caller_uri: url::Url,
    /// All call site ranges within this caller.
    pub ranges: Vec<TextRange>,
}

/// An outgoing call item with aggregated call sites.
///
/// target from within a function body.
#[derive(Clone, Debug)]
pub struct OutgoingCallItem {
    /// Name of the called function.
    pub callee: String,
    /// All call site ranges to this callee.
    pub ranges: Vec<TextRange>,
}

/// A workspace symbol (framework-independent).
///
/// Replaces `lsp_types::WorkspaceSymbol`.
#[derive(Clone, Debug)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub location: FileLocation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_edit_basic() {
        let edit = IdeTextEdit { range: base_db::text_range(0, 5), new_text: "hello".to_string() };
        assert_eq!(base_db::range_len(edit.range), 5);
    }
}
