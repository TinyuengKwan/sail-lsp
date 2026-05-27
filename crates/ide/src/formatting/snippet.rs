//! Source snippet provider — zero-copy access to source text.
//!

use ide_db::line_index::TextRange;

/// Provides access to source code text by TextRange.
pub(crate) struct SnippetProvider {
    source: String,
}

impl SnippetProvider {
    /// Create from owned source text.
    pub(crate) fn new(source: String) -> Self {
        Self { source }
    }

    /// Extract a snippet for a TextRange.
    pub(crate) fn span_to_snippet(&self, range: TextRange) -> &str {
        let start: usize = range.start().into();
        let end: usize = range.end().into();
        &self.source[start..end]
    }

    /// Entire source text.
    pub(crate) fn entire_snippet(&self) -> &str {
        &self.source
    }

    /// Source length in bytes.
    pub(crate) fn len(&self) -> usize {
        self.source.len()
    }
}
