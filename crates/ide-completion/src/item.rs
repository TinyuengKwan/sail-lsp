//! Completion item representation.
//!
//! The `CompletionItem` is the primary output of the completion engine, and
//! it lives here — in the crate that produces it — as rust-analyzer's does.

use ide_db::defs::CompletionItemKind;

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

    // There is no `is_relevant`. Sorting reads `score()`; nothing asks
    // the yes/no question, and a threshold on a score that only orders is
    // a decision nobody made.
}

/// A completion item.
///
/// Shared by providers, analysis and protocol conversion. Declaration origins
/// survive lazy resolution; auto-include candidates carry a request whose
/// additional edits are computed and validated against current inputs.
#[derive(Clone, Debug)]
pub struct CompletionItem {
    /// Requesting file, retained across the LSP completion/resolve round trip.
    pub scope_file: Option<base_db::FileId>,
    /// Selected semantic declaration, retained for lazy documentation.
    pub origin: Option<CompletionOrigin>,
    /// A candidate is resolved against current source/configuration before
    /// publishing its include edit. Kept through the protocol round trip.
    pub import: Option<ide_db::imports::import_assets::ImportRequest>,
    pub additional_text_edits: Vec<ide_db::text_edit::TextEdit>,
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
    /// Physical source range replaced by the primary completion edit.
    pub source_range: Option<ide_db::line_index::TextRange>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub deprecated: bool,
    /// Relevance scoring.
    pub relevance: CompletionRelevance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionOrigin {
    pub file: base_db::FileId,
    pub declaration: usize,
    pub namespace: hir::source_order::bindings::Namespace,
}

impl CompletionItem {
    /// A completion with nothing but a name and a kind.
    ///
    /// rust-analyzer builds every item through `render`, which is why its
    /// `CompletionItem` has a builder rather than nine fields to fill in.
    /// Ours were nine-field literals repeated at every provider — the same
    /// six defaults each time — while `render/` sat beside them with
    /// `#[expect(dead_code)]`.
    pub(crate) fn new(kind: CompletionItemKind, label: impl Into<String>) -> Self {
        Self {
            scope_file: None,
            origin: None,
            import: None,
            additional_text_edits: Vec::new(),
            label: label.into(),
            kind,
            detail: None,
            documentation: None,
            insert_text: None,
            source_range: None,
            sort_text: None,
            filter_text: None,
            deprecated: false,
            relevance: CompletionRelevance::default(),
        }
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn documentation(mut self, doc: impl Into<String>) -> Self {
        self.documentation = Some(doc.into());
        self
    }

    /// What to type in place of the label — a snippet, or a call with its
    /// parentheses already there.
    pub(crate) fn insert_text(mut self, text: impl Into<String>) -> Self {
        self.insert_text = Some(text.into());
        self
    }

    pub(crate) fn sort_text(mut self, text: impl Into<String>) -> Self {
        self.sort_text = Some(text.into());
        self
    }

    pub(crate) fn filter_text(mut self, text: impl Into<String>) -> Self {
        self.filter_text = Some(text.into());
        self
    }

    pub(crate) fn relevance(mut self, relevance: CompletionRelevance) -> Self {
        self.relevance = relevance;
        self
    }
}

/// Deduplicate, score, and sort completion items.
///
/// 1. Deduplicate by label
/// 2. Sort by `relevance.score()` (descending), then alphabetical
/// 3. Assign hex sort_text via XOR-inversion (`score ^ 0xFFFFFFFF`)
/// 4. Truncate to MAX_COMPLETIONS
pub(crate) fn deduplicate_and_sort(items: &mut Vec<CompletionItem>, _prefix: &str) {
    // A type and value can share a spelling, including generated enum names.
    // Locals still take precedence over same-named global values.
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| {
        seen.insert((
            item.label.clone(),
            matches!(
                item.kind,
                CompletionItemKind::Struct
                    | CompletionItemKind::Enum
                    | CompletionItemKind::TypeParameter
            ),
            item.import.as_ref().map(|request| request.target),
        ))
    });

    // Sort by relevance score (descending), then alphabetical
    items.sort_by(|a, b| {
        b.relevance.score().cmp(&a.relevance.score()).then_with(|| a.label.cmp(&b.label))
    });

    // No truncation here: `CompletionConfig::limit` is the one cut, so a
    // client that sets it to 0 gets every candidate, and the server can
    // tell the client when a list was cut.

    // Assign sort_text: invert the score so that higher scores sort first
    // in lexicographic order (`score ^ 0xFFFFFFFF` -> lower hex = better).
    for item in items.iter_mut() {
        let sort_score = item.relevance.score() ^ 0xFF_FF_FF_FF;
        item.sort_text = Some(format!("{sort_score:08x}"));
    }
}
