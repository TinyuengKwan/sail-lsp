//! Foundation database trait for IDE feature modules.
//!
//! give every IDE feature (semantic tokens, completion, hover, code
//! actions, …) a small, abstract view of "a Sail file" so they can
//! live in their own crates without dragging in `sail_server::state::
//! File`'s entire transitive dependency surface.
//!
//! `FileDb` is the seam: a trait whose surface is **only** the methods
//! that real feature modules call. `sail_server::state::File` ships an
//! impl in stage , and the moved feature modules take
//! `&dyn FileDb` instead of `&File`.
//!
//! Future stages add:
//!   - workspace-wide queries (`WorkspaceDb`)
//!   - the `RootDatabase` analogue that owns the file map + caches
//!   - the symbol index (`SymbolIndex`) that powers workspace symbols.

// Module alignment summary (see crates/ide-db/DIFF_NOTES.md for full diff):
//
// ALIGN  — mirrors rust-analyzer's ide-db directly:
//   active_parameter, assists, defs, documentation, fixture, helpers,
//   imports, line_index, prime_caches, rename, root_database, search,
//   source_change, symbol_index, syntax_helpers, text_edit
//
// CUSTOM — sail-lsp additions with no RA counterpart:
//   workspace_index, ide_types, keywords, pragmas, span,
//   type_inference, text_document, db_query
//
// ABSENT — RA-only Rust-specific modules not ported:
//   famous_defs, traits, ty_filter, items_locator, path_transform, label

// WorkspaceFile accessed via SourceFileInfo supertrait chain.
// Re-export SourceFileInfo for hir-ty and other lower crates that need
// text()/item_tree() without depending on ide-db.
pub use hir_def::callgraph::SourceFileInfo;
use parser::{Span, Token};
use std::collections::HashMap;
use syntax::parser_lower::ParsedFile;
use url::Url;

// analysis.rs moved to crates/ide/src/analysis.rs in
pub mod active_parameter;
/// Assist data structures shared between ide-assists and ide-diagnostics.
pub mod assists;
pub mod db_query;
pub mod defs;
pub mod documentation;
#[cfg(any(test, feature = "test-utils"))]
pub mod fixture;
pub mod helpers;
pub mod ide_types;
/// Import management (insert/remove $include, auto-import candidates).
pub mod imports;
pub mod keywords;
pub mod line_index;
pub mod pragmas;
pub mod prime_caches;
pub mod rename;
pub mod root_database;
pub mod search;
pub mod source_change;
pub mod span;
pub mod symbol_index;
/// Syntax tree helpers for IDE features.
pub mod syntax_helpers;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod text_document;
pub mod text_edit;
pub mod workspace_index;
/// Workspace-level abstraction over the file store. Implemented by
/// `sail_server::backend::State`; used by the `Semantics<'_>` facade
/// so it can live in crates/hir without depending on sail_server.
pub trait WorkspaceDb {
    /// Look up a file by URI.
    fn get_file(&self, uri: &Url) -> Option<&dyn FileDb>;
    /// Iterate all known files.
    fn all_files(&self) -> Vec<(&Url, &dyn FileDb)>;
}

pub use keywords::{SAIL_BUILTINS, SAIL_KEYWORDS};

pub use pragmas::KNOWN_PRAGMAS;

pub use line_index::{LineCol, LineIndex, TextRange};
pub use span::{file_location_from_span, text_range_from_span};

pub use helpers::{
    builtin_docs, extract_comments, function_snippet, inlay_param_name, CallableSignature,
    Parameter,
};
pub use prime_caches::ParallelPrimeCachesProgress;
pub use symbol_index::{
    add_definitions, add_parsed_definitions, build_signature_index, collect_callable_signatures,
    collect_callable_signatures_from, document_symbols_ide, extract_symbol_decls,
    find_callable_signature, instantiate_signature, SymbolDecl,
};
pub use text_document::{TextChange, TextChangeRange, TextDocument};
pub use workspace_index::{Query, SearchMode};

/// Bracket / token classifier helpers shared across IDE feature modules.
///
/// These used to live as `pub(crate)` helpers in
/// `sail_server::symbols::analysis`. Promoted to a shared crate-level
/// home in stage so feature modules in `crates/ide` can call them
/// without depending on sail_server.
pub mod token_classify {
    use parser::Token;

    pub fn token_symbol_key(token: &Token) -> Option<String> {
        match token {
            Token::Id(name) => Some(name.clone()),
            Token::TyVal(name) => Some(format!("'{}", name)),
            _ => None,
        }
    }

    pub fn token_is_open_bracket(token: &Token) -> bool {
        matches!(
            token,
            Token::LeftBracket
                | Token::LeftSquareBracket
                | Token::LeftCurlyBracket
                | Token::LeftCurlyBar
                | Token::LeftSquareBar
        )
    }

    pub fn token_is_close_bracket(token: &Token) -> bool {
        matches!(
            token,
            Token::RightBracket
                | Token::RightSquareBracket
                | Token::RightCurlyBracket
                | Token::RightCurlyBar
                | Token::RightSquareBar
        )
    }
}

pub use token_classify::{token_is_close_bracket, token_is_open_bracket, token_symbol_key};

/// Minimal abstract view of a single Sail source file. Implemented by
/// `sail_server::state::File`; used by every moved IDE feature so the
/// feature stays decoupled from the heavy `state::File` type.
///
/// `FileDb` extends [`hir_def::callgraph::WorkspaceFile`], so any feature
/// crate that takes `&dyn FileDb` automatically also gets
/// `content_hash()` and `callgraph()` for free — no extra trait bound
/// at the call site, no per-feature dispatch glue.
///
/// New methods land here only when an actual feature needs them; this
/// trait is intentionally small.
pub trait FileDb: hir_def::callgraph::SourceFileInfo {
    // text() and item_tree() are inherited from SourceFileInfo supertrait.

    /// Line/column position for a 0-based UTF-8 byte offset.
    fn position_at(&self, offset: usize) -> LineCol;

    /// 0-based UTF-8 byte offset for a line/column position.
    fn offset_at(&self, position: &LineCol) -> usize;

    /// Lexed tokens, if the parse pipeline succeeded for this file.
    fn tokens(&self) -> Option<&[(Token, Span)]>;

    /// Token covering `position`, if any. Returned as a borrow into
    /// `tokens()` so callers don't pay clone cost.
    fn token_at(&self, position: LineCol) -> Option<&(Token, Span)>;

    /// Per-file `ParsedFile` index (declarations, call sites,
    /// constructor info). Returns `None` when the parse pipeline
    /// failed for this file.
    fn parsed(&self) -> Option<&ParsedFile>;

    /// Per-file callable bodies (Body arena). Returns `None` when
    /// the parse pipeline failed for this file.
    fn bodies(&self) -> Option<&hir_def::bodies::CallableBodies> {
        None
    }

    // TODO(I): These perform semantic analysis or aggregate workspace
    // data. New code should prefer Semantics methods; these remain
    // on FileDb for backwards compatibility until all consumers are
    // migrated.

    /// Per-file signature index keyed by callable name. Returns
    /// `None` for files where the signature index hasn't been
    /// built yet.
    fn signature_index(&self) -> Option<&HashMap<String, CallableSignature>>;

    /// Per-file find-references count map keyed by symbol name.
    fn ref_counts(&self) -> &HashMap<String, usize>;

    /// Per-file find-implementations count map keyed by symbol name.
    fn impl_counts(&self) -> &HashMap<String, usize>;

    /// Type-text for the binding declared at `span`, when typecheck
    /// information is available. Default impl returns `None`, so
    /// feature crates that don't care about typed bindings (and
    /// in-test stand-ins) get the right answer for free; only
    /// `sail_server::state::File` overrides it to forward to the
    /// real typechecker output.
    fn binding_type_text(&self, _span: Span) -> Option<String> {
        None
    }

    /// Cached type-text for the expression at `span`, if a previous
    /// typecheck pass produced type information for this file.
    /// Default impl returns `None`; sail_server overrides with a
    /// lookup into `TypeCheckResult::expr_type_text`.
    fn cached_expr_type_text(&self, _span: Span) -> Option<String> {
        None
    }
}
