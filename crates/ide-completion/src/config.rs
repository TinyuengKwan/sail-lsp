//! Completion configuration.

/// How to render callable completions (functions, methods).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallableSnippets {
    /// Insert `fn_name($0)` — place cursor inside parens.
    FillArguments,
    /// Insert `fn_name(${1:arg1}, ${2:arg2})` — tabstop per arg.
    AddParentheses,
}

/// Which completion response fields should be resolved lazily via `completionItem/resolve`.
#[derive(Clone, Debug, Default)]
pub struct CompletionFieldsToResolve {
    /// Resolve `detail` lazily.
    pub resolve_detail: bool,
    /// Resolve `documentation` lazily.
    pub resolve_documentation: bool,
    /// Resolve `filterText` lazily.
    pub resolve_filter_text: bool,
    /// Resolve `textEdit` / `additionalTextEdits` lazily.
    pub resolve_text_edit: bool,
    /// Resolve `command` lazily.
    pub resolve_command: bool,
}

/// Configuration for the completion engine.
#[derive(Clone, Debug)]
pub struct CompletionConfig {
    /// Enable postfix completions (e.g., `expr.if`, `expr.match`).
    pub enable_postfix_completions: bool,
    /// Enable keyword completions.
    pub enable_keyword_completions: bool,
    /// Enable snippet completions (code templates).
    pub enable_snippets: bool,
    /// Enable auto-import completions (flyimport).
    ///
    /// Sail: auto-adds `$include` for unresolved cross-file symbols.
    pub enable_imports_on_the_fly: bool,
    /// Maximum number of completion items to return.
    pub limit: Option<usize>,
    /// Whether the client supports snippet syntax.
    pub snippet_cap: Option<SnippetCap>,
    /// Show full function signatures in completion detail.
    pub full_function_signatures: bool,
    /// How to render callable (function/method) completions.
    pub callable: Option<CallableSnippets>,
    /// Whether to add a semicolon after completing a unit-returning function.
    pub add_semicolon_to_unit: bool,
    /// Which fields should be resolved lazily by the client.
    pub fields_to_resolve: CompletionFieldsToResolve,
}

/// Marker for snippet support capability.
#[derive(Clone, Copy, Debug)]
pub struct SnippetCap {
    _private: (),
}

impl SnippetCap {
    /// Create if the client supports snippets.
    pub fn new(supports_snippets: bool) -> Option<Self> {
        if supports_snippets {
            Some(SnippetCap { _private: () })
        } else {
            None
        }
    }
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enable_postfix_completions: true,
            enable_keyword_completions: true,
            enable_snippets: true,
            enable_imports_on_the_fly: false,
            limit: Some(200),
            snippet_cap: None,
            full_function_signatures: false,
            callable: Some(CallableSnippets::FillArguments),
            add_semicolon_to_unit: false,
            fields_to_resolve: CompletionFieldsToResolve::default(),
        }
    }
}
