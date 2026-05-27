//! Assist configuration.

use ide_db::assists::AssistKind;

/// Marker type for snippet support. `None` means the client does not support
/// snippet text edits.
#[derive(Clone, Copy, Debug)]
pub struct SnippetCap(());

impl SnippetCap {
    /// Returns `Some(SnippetCap)` if the client advertises snippet support.
    pub fn new(client_supports: bool) -> Option<Self> {
        if client_supports {
            Some(Self(()))
        } else {
            None
        }
    }
}

/// How to fill default expressions in generated code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExprFillDefaultMode {
    /// Use `todo!()` (Rust) or `undefined` (Sail).
    Todo,
    /// Use default value (unit, zero, etc.).
    Default,
}

/// Configuration for assist generation.
#[derive(Clone, Debug)]
pub struct AssistConfig {
    /// If set, only assists of these kinds are returned.
    pub allowed: Option<Vec<AssistKind>>,
    /// Configuration for `$include` insertion.
    pub insert_use: ide_db::imports::insert_use::InsertUseConfig,
    /// Snippet support (for tabstops in generated code).
    pub snippet_cap: Option<SnippetCap>,
    /// Whether to group related code actions.
    pub code_action_grouping: bool,
    /// How to fill missing expression bodies (e.g., `todo!()` vs `()`).
    pub expr_fill_default: ExprFillDefaultMode,
}

impl Default for AssistConfig {
    fn default() -> Self {
        Self {
            allowed: None,
            insert_use: Default::default(),
            snippet_cap: None,
            code_action_grouping: true,
            expr_fill_default: ExprFillDefaultMode::Todo,
        }
    }
}

impl AssistConfig {
    /// Check whether an assist of the given kind is allowed.
    pub fn is_allowed(&self, kind: AssistKind) -> bool {
        match &self.allowed {
            Some(allowed) => allowed.iter().any(|k| k.contains(kind)),
            None => true,
        }
    }
}
