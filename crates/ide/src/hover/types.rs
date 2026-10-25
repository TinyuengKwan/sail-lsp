//! Hover's result types.
//!
//! rust-analyzer keeps a feature's result type in the module that produces
//! it — `HoverResult` in `ide/src/hover.rs`, `InlayHint` in
//! `ide/src/inlay_hints.rs`, `HlRange` in `ide/src/syntax_highlighting.rs`.
//! These lived one crate below, in `ide-db/src/ide_types.rs`, together with
//! forty other types: a crate that cannot see `ide` cannot name what `ide`
//! knows, which is why `HoverFilePosition` exists beside `ide::FilePosition`
//! and why `CompletionItem` was defined twice.

use crate::navigation_target::NavigationTarget;
use ide_db::line_index::TextRange;

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
    /// Documentation format.
    pub format: HoverDocFormat,
    /// Max struct/union fields to show.
    pub max_fields_count: Option<usize>,
    /// Max enum variants to show.
    pub max_enum_variants_count: Option<usize>,
    /// Maximum overload alternatives to display; None shows all.
    pub max_overloads_count: Option<usize>,
}

impl Default for HoverConfig {
    fn default() -> Self {
        Self {
            documentation: true,
            format: HoverDocFormat::Markdown,
            max_fields_count: Some(10),
            max_enum_variants_count: Some(10),
            max_overloads_count: Some(10),
        }
    }
}
