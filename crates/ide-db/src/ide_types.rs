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

/// A navigation target: where the user can "jump to".
///
/// ```text
/// pub struct NavigationTarget {
///     pub file_id: FileId,
///     pub full_range: TextRange,
///     pub focus_range: Option<TextRange>,
///     pub name: SmolStr,
///     pub kind: Option<SymbolKind>,
/// }
/// ```
///
/// Sail extension: includes `url` for backward compatibility with
/// callers that haven't migrated to `FileId` yet.
#[derive(Clone, Debug)]
pub struct NavigationTarget {
    /// File containing this target.
    ///
    /// Optional for backward compat — will become required.
    pub file_id: Option<base_db::FileId>,
    /// URL of the file (backward compat, will be derived from file_id).
    pub url: url::Url,
    /// Name of the symbol.
    pub name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Full range of the entire definition.
    pub full_range: TextRange,
    /// Range of just the name/focus portion.
    pub focus_range: TextRange,
    /// Additional detail (type signature, etc.).
    pub detail: Option<String>,
    /// Documentation string.
    pub docs: Option<String>,
    /// Nested targets (e.g., struct fields inside a struct).
    pub children: Vec<NavigationTarget>,
}

/// SymbolKind canonical definition moved to defs.rs.
pub use crate::defs::SymbolKind;

/// Hover result with structured markup and actions.
///
/// ```text
/// pub struct HoverResult {
///     pub markup: Markup,
///     pub actions: Vec<HoverAction>,
/// }
/// ```
///
/// Extended with `range` for LSP response.
#[derive(Clone, Debug, Default)]
pub struct HoverResult {
    /// Rich text content (markdown).
    pub markup: String,
    /// Actions attached to this hover (go to impl, go to type, etc.).
    pub actions: Vec<HoverAction>,
    /// Byte range this hover applies to (for LSP response).
    pub range: TextRange,
}

/// Actions that can be triggered from a hover popup.
///
/// ```text
/// pub enum HoverAction {
///     Runnable(Runnable),
///     Implementation(FilePosition),
///     Reference(FilePosition),
///     GoToType(Vec<HoverGotoTypeData>),
/// }
/// ```
///
/// No Runnable variant (Sail has no test runner integration).
#[derive(Clone, Debug)]
pub enum HoverAction {
    /// "Go to implementations" (for val specs, scattered defs).
    Implementation(HoverFilePosition),
    /// "Go to references".
    Reference(HoverFilePosition),
    /// "Go to type definition" with navigation data.
    GoToType(Vec<HoverGotoTypeData>),
}

/// A position in a file, used inside HoverAction.
///
/// Separate from `ide::FilePosition` (which uses `rowan::TextSize`)
/// because ide-db cannot depend on ide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverFilePosition {
    pub file_id: base_db::FileId,
    pub offset: usize,
}

/// Navigation data for "go to type" hover action.
///
/// ```text
/// pub struct HoverGotoTypeData {
///     pub mod_path: String,
///     pub nav: NavigationTarget,
/// }
/// ```
#[derive(Clone, Debug)]
pub struct HoverGotoTypeData {
    /// Module path (e.g., "model/types.sail::Foo").
    pub mod_path: String,
    /// Navigation target for the type.
    pub nav: NavigationTarget,
}

/// Hover documentation format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverDocFormat {
    Markdown,
    PlainText,
}

/// Configuration for hover behavior.
///
#[derive(Clone, Debug)]
pub struct HoverConfig {
    /// Show documentation in hover.
    pub documentation: bool,
    /// Show keyword documentation.
    pub keywords: bool,
    /// Documentation format.
    pub format: HoverDocFormat,
    /// Max struct/union fields to show.
    pub max_fields_count: Option<usize>,
    /// Max enum variants to show.
    pub max_enum_variants_count: Option<usize>,
}

impl Default for HoverConfig {
    fn default() -> Self {
        Self {
            documentation: true,
            keywords: true,
            format: HoverDocFormat::Markdown,
            max_fields_count: None,
            max_enum_variants_count: None,
        }
    }
}

/// An inlay hint at a byte offset.
///
/// Replaces `lsp_types::InlayHint`. Uses byte offset, not Position.
#[derive(Clone, Debug)]
pub struct InlayHint {
    pub offset: usize,
    pub label: String,
    pub kind: InlayHintKind,
    pub tooltip: Option<String>,
    pub padding_left: Option<bool>,
    pub padding_right: Option<bool>,
    pub data: Option<serde_json::Value>,
}

/// Inlay hint kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlayHintKind {
    Type,
    Parameter,
    Other,
}

/// A single semantic token (delta-encoded, framework-independent).
///
/// Replaces `lsp_types::SemanticToken`. Same fields — the LSP
/// protocol defines the encoding, not the framework.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlRange {
    pub delta_line: u32,
    pub delta_start: u32,
    pub length: u32,
    pub token_type: u32,
    pub token_modifiers_bitset: u32,
}

/// A batch of semantic tokens with an optional result ID for delta support.
///
/// Replaces `lsp_types::SemanticTokens`.
#[derive(Clone, Debug)]
pub struct HlRanges {
    pub result_id: Option<String>,
    pub data: Vec<HlRange>,
}

/// A delta update to a previous set of semantic tokens.
///
/// Replaces `lsp_types::SemanticTokensDelta`.
#[derive(Clone, Debug)]
pub struct HlRangesDelta {
    pub result_id: Option<String>,
    pub edits: Vec<HlRangesEdit>,
}

/// A single edit within a semantic tokens delta.
///
/// Replaces `lsp_types::SemanticTokensEdit`.
#[derive(Clone, Debug)]
pub struct HlRangesEdit {
    pub start: u32,
    pub delete_count: u32,
    pub data: Option<Vec<HlRange>>,
}

/// Relevance scoring for completion items.
///
/// Each field adjusts the item's score relative to BASE_SCORE.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompletionRelevance {
    /// Identifier name exactly matches expected parameter/variable name.
    pub exact_name_match: bool,
    /// Type matches the expected type at cursor position.
    pub type_match: Option<CompletionRelevanceTypeMatch>,
    /// Item is a local variable (not workspace-level).
    pub is_local: bool,
    /// Exact postfix template match.
    pub postfix_match: Option<CompletionRelevancePostfixMatch>,
    /// Item requires cross-file resolution (e.g., from another $include).
    pub requires_import: bool,
    /// Item is a private field but editable (e.g., same module).
    pub is_private_editable: bool,
    /// Item is deprecated.
    pub is_deprecated: bool,
    /// Function-specific relevance factors.
    pub function: Option<CompletionRelevanceFn>,
}

/// Type match quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionRelevanceTypeMatch {
    /// Type could unify with expected (generic match).
    CouldUnify,
    /// Exact type match.
    Exact,
}

/// Postfix completion match quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionRelevancePostfixMatch {
    /// Postfix trigger exists but isn't an exact match.
    NonExact,
    /// Exact postfix match (e.g., `.if` on a boolean expression).
    Exact,
}

/// Function-specific relevance factors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRelevanceFn {
    /// Function takes at least one parameter.
    pub has_params: bool,
    /// Function has a self/receiver parameter.
    pub has_self_param: bool,
    /// How the return type relates to the expected type.
    pub return_type: CompletionRelevanceReturnType,
}

/// How a function's return type relates to the expected type at cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompletionRelevanceReturnType {
    /// No special relationship.
    #[default]
    Other,
    /// Returns the type being constructed (e.g., `Vec::new() -> Vec`).
    DirectConstructor,
    /// Returns a wrapper containing the type (e.g., `Some(x) -> option(x)`).
    Constructor,
    /// Returns Self or &mut Self (builder pattern).
    Builder,
}

impl CompletionRelevance {
    /// Base score — midpoint of u32 range.
    const BASE_SCORE: u32 = u32::MAX / 2;

    /// Compute the numeric score for sorting.
    ///
    /// Higher score = more relevant = sorted first.
    pub fn score(&self) -> u32 {
        let mut score = Self::BASE_SCORE;

        if self.exact_name_match {
            score += 20;
        }
        if self.is_local {
            score += 3;
        }
        if !self.is_private_editable {
            score += 1;
        }
        if self.requires_import {
            score = score.saturating_sub(5);
        }
        if self.is_deprecated {
            score = score.saturating_sub(3);
        }

        match self.postfix_match {
            Some(CompletionRelevancePostfixMatch::Exact) => score += 100,
            Some(CompletionRelevancePostfixMatch::NonExact) => {
                score = score.saturating_sub(5);
            }
            None => {}
        }

        match self.type_match {
            Some(CompletionRelevanceTypeMatch::Exact) => score += 18,
            Some(CompletionRelevanceTypeMatch::CouldUnify) => score += 5,
            None => {}
        }

        // Function-specific factors
        if let Some(ref fn_rel) = self.function {
            match fn_rel.return_type {
                CompletionRelevanceReturnType::DirectConstructor => score += 15,
                CompletionRelevanceReturnType::Builder => score += 10,
                CompletionRelevanceReturnType::Constructor => score += 5,
                CompletionRelevanceReturnType::Other => {}
            }
            if fn_rel.has_params {
                score = score.saturating_sub(1);
            }
            if fn_rel.has_self_param {
                score = score.max(1);
            }
        }

        score
    }

    /// Whether the item is considered "relevant" (score above base).
    pub fn is_relevant(&self) -> bool {
        self.score() > Self::BASE_SCORE
    }
}

/// A completion item (framework-independent).
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
    pub text_edit: Option<IdeTextEdit>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub deprecated: bool,
    /// Relevance scoring.
    pub relevance: CompletionRelevance,
}

/// CompletionItemKind canonical definition moved to defs.rs.
pub use crate::defs::CompletionItemKind;

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

/// A folding range (byte offsets).
#[derive(Clone, Debug)]
pub struct FoldingRange {
    pub range: TextRange,
    pub kind: FoldingRangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldingRangeKind {
    Region,
    Comment,
    Import,
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

    #[test]
    fn navigation_target_basic() {
        let target = NavigationTarget {
            file_id: None,
            url: url::Url::parse("file:///test.sail").unwrap(),
            name: "foo".to_string(),
            kind: SymbolKind::Function,
            full_range: base_db::text_range(0, 20),
            focus_range: base_db::text_range(9, 12),
            detail: Some("int -> int".to_string()),
            docs: None,
            children: vec![],
        };
        assert_eq!(target.name, "foo");
    }
}
