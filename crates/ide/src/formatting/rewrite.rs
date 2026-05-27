//! Rewrite context — shared state for formatting operations.
//!
//! `RewriteContext` carries config + source access through the pipeline.
//! `RewriteResult` is the return type for formatting functions.

use ide_db::line_index::TextRange;

use super::snippet::SnippetProvider;
use super::FormatOptions;

/// Result of a rewrite operation.
pub(crate) type RewriteResult = Result<String, String>;

/// Shared context passed to all rewrite functions.
pub(crate) struct RewriteContext<'a> {
    /// Formatting configuration.
    pub(crate) config: &'a FormatOptions,
    /// Source text access.
    pub(crate) snippet_provider: &'a SnippetProvider,
}

impl<'a> RewriteContext<'a> {
    /// Create a new context.
    pub(crate) fn new(config: &'a FormatOptions, snippet_provider: &'a SnippetProvider) -> Self {
        Self { config, snippet_provider }
    }

    /// Get source text for a range.
    pub(crate) fn snippet(&self, range: TextRange) -> &str {
        self.snippet_provider.span_to_snippet(range)
    }

    /// Remaining budget after used_width.
    pub(crate) fn budget(&self, used_width: usize) -> usize {
        self.config.max_width().saturating_sub(used_width)
    }
}
