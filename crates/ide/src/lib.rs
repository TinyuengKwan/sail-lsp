//! High-level IDE feature entry points.
//!
//! feature (semantic tokens, formatting, hover, completion, code
//! actions, ...) that takes an abstract `&dyn ide_db::FileDb` instead
//! of a heavy `sail_server::state::File`.
//!
//! # ALIGN / CUSTOM status (vs rust-analyzer `ide`)
//!
//! **ALIGN** — modules present in both sail-lsp and RA with equivalent roles:
//! `analysis` (`Analysis`/`AnalysisHost` snapshot pattern), `annotations`,
//! `call_hierarchy`, `calls`, `completion`, `doc_links`, `extend_selection`,
//! `file_structure`, `folding_ranges`, `goto_declaration`, `goto_definition`,
//! `goto_implementation`, `goto_type_definition`, `highlight_related`, `hover`,
//! `inlay_hints`, `join_lines`, `markup`, `matching_brace`, `moniker`,
//! `move_item`, `navigation_target`, `references`, `rename`, `signature_help`,
//! `ssr`, `static_index`, `syntax_highlighting`, `typing`,
//! `view_hir`, `view_item_tree`, `view_syntax_tree`.
//!
//! **CUSTOM** — Sail-only modules with no RA counterpart:
//! `bitfield_layout` (field-offset hover table),
//! `effect_annotations` (per-callable effect code lens),
//! `expand_include` (`$include` inline expansion),
//! `include_graph_view` (`$include` dependency graph renderer),
//! `formatting` (consolidated Sail formatting + join_lines + linked_editing),
//! `navigation` (scattered-clause and include-graph navigation helpers).
//!
//! See `crates/ide/DIFF_NOTES.md` for the full file-count comparison.

pub mod analysis;
pub mod annotations;
pub mod bitfield_layout;
pub mod call_hierarchy;
pub mod calls;
pub mod completion;
pub mod doc_links;
pub mod effect_annotations;
pub mod expand_include;
pub mod extend_selection;
pub mod file_structure;
pub mod folding_ranges;
pub mod formatting;
pub mod goto_declaration;
pub mod goto_definition;
pub mod goto_implementation;
pub mod goto_type_definition;
pub mod highlight_related;
pub mod hover;
pub mod include_graph_view;
pub mod inlay_hints;
pub mod join_lines;
mod markdown_remove;
/// Rich text formatting for hover/docs.
pub mod markup;
pub mod matching_brace;
pub mod moniker;
pub mod move_item;
pub mod navigation;
/// Unified navigation target type.
pub mod navigation_target;
pub mod references;
pub mod rename;
pub mod runnables;
pub mod signature_help;
pub mod ssr;
pub mod static_index;
pub mod syntax_highlighting;
pub mod typing;
pub mod view_hir;
pub mod view_item_tree;
pub mod view_syntax_tree;

pub use analysis::{Analysis, AnalysisHost};
pub use markup::Markup;
pub use navigation_target::{NavigationTarget, ToNav, TryToNav};

/// A position in a file: file + byte offset.
///
/// Server layer converts LSP `TextDocumentPositionParams` to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePosition {
    pub file_id: base_db::FileId,
    pub offset: rowan::TextSize,
}

/// A range in a file: file + text range.
///
/// Server layer converts LSP `TextDocumentIdentifier + Range` to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileRange {
    pub file_id: base_db::FileId,
    pub range: rowan::TextRange,
}

/// A value annotated with the range it was computed from.
///
/// Used as return type for hover, goto-def, etc.
#[derive(Debug, Clone)]
pub struct RangeInfo<T> {
    pub range: rowan::TextRange,
    pub info: T,
}

impl<T> RangeInfo<T> {
    pub fn new(range: rowan::TextRange, info: T) -> RangeInfo<T> {
        RangeInfo { range, info }
    }
}

/// Type alias for cancellable results.
///
/// Re-exported from ide-db for convenience.
pub type Cancellable<T> = ide_db::ide_types::Cancellable<T>;

pub use formatting::{
    document_links_for_file, format_document_edits, join_lines_edits,
    linked_editing_ranges_for_position, make_selection_range, matching_brace_offset,
    on_enter_edits, range_format_document_edits,
};
// Move item (replaces legacy move_item_edits/MoveDirection)
pub use move_item::{move_item, Direction as MoveDirection};
pub use syntax_highlighting::{
    compute_semantic_tokens, compute_semantic_tokens_delta, compute_semantic_tokens_range,
};
