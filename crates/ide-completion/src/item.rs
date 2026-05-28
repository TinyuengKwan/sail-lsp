//! Completion item representation.
//! The `CompletionItem` is the primary output of the completion engine.
//! It contains all information needed to render a completion suggestion
//! in the editor.

use ide_db::defs::CompletionItemKind;
use ide_db::documentation::Documentation;
use ide_db::ide_types::CompletionRelevance;
use ide_db::line_index::TextRange;

/// Structured label for a completion item.
///
/// ```text
/// pub struct CompletionItemLabel {
///     pub primary: SmolStr,
///     pub detail_left: Option<String>,
///     pub detail_right: Option<String>,
/// }
/// ```
///
/// `detail_left` renders next to the label (e.g., return type);
/// `detail_right` renders on the far right (e.g., source module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItemLabel {
    /// Primary display text (e.g., function name).
    pub primary: String,
    /// Detail shown left of the label (e.g., `: int`).
    pub detail_left: Option<String>,
    /// Detail shown right of the label (e.g., source file).
    pub detail_right: Option<String>,
}

impl CompletionItemLabel {
    pub fn new(primary: impl Into<String>) -> Self {
        Self { primary: primary.into(), detail_left: None, detail_right: None }
    }
}

impl From<String> for CompletionItemLabel {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for CompletionItemLabel {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// A single completion suggestion.
/// This is the ide-completion-internal representation. The LSP layer
/// converts it to `lsp_types::CompletionItem` via `to_proto`.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Structured label with primary text + detail annotations.
    pub label: CompletionItemLabel,
    /// The range of source text to replace when applying.
    pub source_range: TextRange,
    /// The text to insert (may differ from label for snippets).
    pub text_edit: Option<String>,
    /// Whether this is a snippet (contains `$0`, `$1`, etc.).
    pub is_snippet: bool,
    /// The kind of completion (function, variable, keyword, etc.).
    pub kind: CompletionItemKind,
    /// Alternative lookup text (for fuzzy matching).
    pub lookup: Option<String>,
    /// Short detail string shown alongside label.
    pub detail: Option<String>,
    /// Documentation shown in popup.
    pub documentation: Option<Documentation>,
    /// Whether this item is deprecated.
    pub deprecated: bool,
    /// Whether completing this item should trigger parameter info.
    pub trigger_call_info: bool,
    /// Relevance scoring for sorting.
    pub relevance: CompletionRelevance,
    /// Text used for sorting (hex-encoded relevance score).
    pub sort_text: Option<String>,
}

impl Default for CompletionItem {
    fn default() -> Self {
        Self {
            label: CompletionItemLabel::new(""),
            source_range: base_db::text_range(0, 0),
            text_edit: None,
            is_snippet: false,
            kind: CompletionItemKind::Variable,
            lookup: None,
            detail: None,
            documentation: None,
            deprecated: false,
            trigger_call_info: false,
            relevance: CompletionRelevance::default(),
            sort_text: None,
        }
    }
}

impl CompletionItem {
    /// Create a new completion item with minimal fields.
    pub fn new(
        kind: CompletionItemKind,
        source_range: TextRange,
        label: impl Into<String>,
    ) -> Self {
        Self { label: CompletionItemLabel::new(label), source_range, kind, ..Default::default() }
    }

    /// The primary label text (convenience accessor).
    pub fn label_text(&self) -> &str {
        &self.label.primary
    }

    /// Builder method: set the detail string.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Builder method: set the text edit.
    pub fn text_edit(mut self, text: impl Into<String>) -> Self {
        self.text_edit = Some(text.into());
        self
    }

    /// Builder method: set documentation.
    pub fn documentation(mut self, doc: Documentation) -> Self {
        self.documentation = Some(doc);
        self
    }

    /// Builder method: set documentation from string (convenience).
    pub fn documentation_str(mut self, doc: impl Into<String>) -> Self {
        self.documentation = Some(Documentation::new(doc.into()));
        self
    }

    /// Builder method: set relevance.
    pub fn set_relevance(mut self, relevance: CompletionRelevance) -> Self {
        self.relevance = relevance;
        self
    }

    /// Builder method: mark as deprecated.
    pub fn set_deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = deprecated;
        self
    }

    /// Builder method: set lookup text.
    pub fn lookup_by(mut self, lookup: impl Into<String>) -> Self {
        self.lookup = Some(lookup.into());
        self
    }

    /// Builder method: mark as snippet.
    pub fn snippet(mut self, snippet_text: impl Into<String>) -> Self {
        self.text_edit = Some(snippet_text.into());
        self.is_snippet = true;
        self
    }

    /// Builder method: set label detail_left.
    pub fn label_detail_left(mut self, detail: impl Into<String>) -> Self {
        self.label.detail_left = Some(detail.into());
        self
    }

    /// Builder method: set label detail_right.
    pub fn label_detail_right(mut self, detail: impl Into<String>) -> Self {
        self.label.detail_right = Some(detail.into());
        self
    }
}

/// Convert from legacy IdeCompletionItem to the CompletionItem.
///
/// This allows providers still using the old type to feed into the
/// Completions accumulator. Will be removed when all providers are migrated.
impl From<ide_db::ide_types::CompletionItem> for CompletionItem {
    fn from(old: ide_db::ide_types::CompletionItem) -> Self {
        Self {
            label: CompletionItemLabel::new(&old.label),
            source_range: base_db::text_range(0, 0), // placeholder — providers should set this
            text_edit: old.insert_text,
            is_snippet: false,
            kind: old.kind,
            lookup: old.filter_text,
            detail: old.detail,
            documentation: old.documentation.map(Documentation::new),
            deprecated: old.deprecated,
            trigger_call_info: false,
            relevance: old.relevance,
            sort_text: old.sort_text,
        }
    }
}

impl CompletionItem {
    /// Convert to the IDE-layer completion item type.
    ///
    /// Bridges ide-completion's CompletionItem to
    /// ide-db's IdeCompletionItem (LSP layer).
    pub fn to_ide(&self) -> ide_db::ide_types::CompletionItem {
        ide_db::ide_types::CompletionItem {
            label: self.label.primary.clone(),
            kind: self.kind,
            detail: self.detail.clone(),
            documentation: self.documentation.as_ref().map(|d| d.as_str().to_owned()),
            insert_text: self.text_edit.clone(),
            text_edit: None,
            sort_text: self.sort_text.clone(),
            filter_text: self.lookup.clone(),
            deprecated: self.deprecated,
            relevance: self.relevance,
        }
    }
}
