//! Assist data structures shared between `ide-assists` and `ide-diagnostics`.
//! The core assist types live here (in ide-db) rather than in ide-assists
//! so that both ide-assists and ide-diagnostics can attach fix actions
//! without creating a circular dependency.

use crate::line_index::TextRange;
use crate::source_change::SourceChange;

/// Identifies an assist and its kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssistId(pub &'static str, pub AssistKind);

/// Kind of assist — maps to LSP `CodeActionKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssistKind {
    /// None — a plain code action.
    None,
    /// Quick fix for a diagnostic.
    QuickFix,
    /// General refactoring.
    Refactor,
    /// Extract code into a new entity.
    RefactorExtract,
    /// Inline code from a reference.
    RefactorInline,
    /// Rewrite code in a different form.
    RefactorRewrite,
    /// Source-level organization.
    Source,
    /// Code generation.
    Generate,
}

impl AssistKind {
    /// Whether `self` is a sub-kind of `other`.
    pub fn contains(self, other: AssistKind) -> bool {
        if self == other {
            return true;
        }
        match self {
            AssistKind::Refactor => matches!(
                other,
                AssistKind::RefactorExtract
                    | AssistKind::RefactorInline
                    | AssistKind::RefactorRewrite
            ),
            _ => false,
        }
    }
}

/// A single assist (code action).
#[derive(Clone, Debug)]
pub struct Assist {
    pub id: AssistId,
    /// Short, human-readable label.
    pub label: Label,
    /// Group for related assists (e.g., "Extract" group).
    pub group: Option<GroupLabel>,
    /// The range of source code this assist targets.
    pub target: TextRange,
    /// The actual code change, if computed.
    pub source_change: Option<SourceChange>,
    /// Optional command to run after applying (e.g., trigger rename).
    pub command: Option<Command>,
    /// Direct text edits (Sail bridge — handlers produce these directly).
    /// Will be migrated to `source_change` as handlers adopt SourceChangeBuilder.
    pub edits: Vec<crate::text_edit::TextEdit>,
}

/// Label for an assist.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label(pub String);

impl Label {
    pub fn new(s: impl Into<String>) -> Self {
        Label(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Group label for related assists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroupLabel(pub String);

/// Post-assist command.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Trigger parameter hints after applying.
    TriggerParameterHints,
    /// Trigger rename after applying.
    Rename,
}

/// Controls which assists get their `source_change` resolved eagerly.
///
/// Used by diagnostics and assists to avoid computing edits until
/// the client explicitly requests them.
#[derive(Clone, Debug)]
pub enum AssistResolveStrategy {
    /// Do not resolve any assists.
    None,
    /// Resolve all assists eagerly.
    All,
    /// Resolve only a specific assist.
    Single(SingleResolve),
}

/// Identifies a single assist to resolve.
#[derive(Clone, Debug)]
pub struct SingleResolve {
    pub assist_id: String,
    pub assist_kind: AssistKind,
}

impl AssistResolveStrategy {
    /// Whether an assist with the given id should be resolved.
    pub fn should_resolve(&self, id: &AssistId) -> bool {
        match self {
            AssistResolveStrategy::None => false,
            AssistResolveStrategy::All => true,
            AssistResolveStrategy::Single(single) => {
                single.assist_id == id.0 && single.assist_kind.contains(id.1)
            }
        }
    }
}

/// Marker type indicating the client supports snippet text edits.
///
/// Only created via `new()` when the client declares snippet support.
#[derive(Clone, Copy, Debug)]
pub struct SnippetCap {
    _private: (),
}

impl SnippetCap {
    /// Create a new `SnippetCap`, asserting that the client supports snippets.
    pub const fn new(client_supports_snippets: bool) -> Option<Self> {
        if client_supports_snippets {
            Some(Self { _private: () })
        } else {
            Option::None
        }
    }
}
