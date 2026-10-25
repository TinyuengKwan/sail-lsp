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
//   assists, defs, documentation, fixture, helpers,
//   imports, line_index, prime_caches, rename, root_database, search,
//   source_change, symbol_index, signatures, syntax_helpers, text_edit
//
// CUSTOM — sail-lsp additions with no RA counterpart:
//   signatures, ide_types, keywords, pragmas, span,
//   type_inference, text_document, db_query
//
// ABSENT — RA-only Rust-specific modules not ported:
//   famous_defs, traits, ty_filter, items_locator, path_transform, label

// WorkspaceFile accessed via SourceFileInfo supertrait chain.
// Re-export SourceFileInfo for hir-ty and other lower crates that need
// text()/item_tree() without depending on ide-db.
pub use hir::callgraph::SourceFileInfo;
use parser::{Span, Token};
use syntax::parser_lower::ParsedFile;
use url::Url;

// analysis.rs moved to crates/ide/src/analysis.rs in
#[cfg(any(test, feature = "test-utils"))]
pub mod annotations;
pub mod apply_change;
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
#[cfg(test)]
mod ordered_inference_tests;
pub mod pragmas;
pub mod prime_caches;
pub mod rename;
pub mod root_database;
pub mod search;
pub mod signatures;
pub mod source_change;
pub mod span;
/// The workspace's symbols, by name — rust-analyzer's `symbol_index`, and
/// its role. It was `workspace_index` while `symbol_index` held
/// callable-signature collection, so the name that matches RA's was on the
/// module that does not do RA's job.
pub mod symbol_index;
/// Syntax tree helpers for IDE features.
pub mod syntax_helpers;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod text_document;
pub mod text_edit;
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

pub use helpers::{builtin_docs, function_snippet, inlay_param_name, CallableSignature, Parameter};
pub use prime_caches::ParallelPrimeCachesProgress;
pub use signatures::{
    add_definitions, collect_callable_signatures, collect_callable_signatures_from,
    extract_symbol_decls, find_callable_signature, instantiate_signature, SymbolDecl,
};
pub use symbol_index::{Query, SearchMode};
pub use text_document::{TextChange, TextChangeRange, TextDocument};

/// Bracket / token classifier helpers shared across IDE feature modules.
///
/// These used to live as `pub(crate)` helpers in
/// `sail_server::symbols::analysis`. Promoted to a shared crate-level
/// home in stage so feature modules in `crates/ide` can call them
/// without depending on sail_server.
pub mod token_classify {
    use parser::Token;

    /// The name an IDE feature should look a token up by.
    ///
    /// Drives goto-definition, goto-declaration, hover, references, rename
    /// and call hierarchy, so a token missing here is invisible to all of
    /// them.
    pub fn token_symbol_key(token: &Token) -> Option<String> {
        match token {
            Token::Id(name) => Some(name.clone()),
            // The lexer keeps the leading apostrophe in the payload, so it
            // must not be added again — `'n` was being looked up as `''n`.
            Token::TyVal(name) => Some(name.clone()),
            // A user-defined operator is an ordinary name: `overload
            // operator =^ = {sail_mask}` declares `=^`.
            Token::InfixOp(op) => Some(op.clone()),
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
                // The lexer folds the opening brace of a structured pragma
                // into the introducer: `$c_override {` is one token.
                | Token::StructuredDirectiveStart(_)
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

    #[cfg(test)]
    mod tests {
        use super::token_symbol_key;
        use parser::Token;

        #[test]
        fn type_variable_key_has_one_apostrophe() {
            // `parser::tokenize` yields the apostrophe as part of the payload.
            let tokens = parser::tokenize("'n");
            assert_eq!(tokens[0].0, Token::TyVal("'n".to_owned()));
            assert_eq!(token_symbol_key(&tokens[0].0).as_deref(), Some("'n"));
        }

        #[test]
        fn user_defined_operator_has_a_key() {
            let tokens = parser::tokenize("=^");
            assert_eq!(tokens[0].0, Token::InfixOp("=^".to_owned()));
            assert_eq!(token_symbol_key(&tokens[0].0).as_deref(), Some("=^"));
        }
    }
}

pub use token_classify::{token_is_close_bracket, token_is_open_bracket, token_symbol_key};

/// Minimal abstract view of a single Sail source file. Implemented by
/// `sail_server::state::File`; used by every moved IDE feature so the
/// feature stays decoupled from the heavy `state::File` type.
///
/// `FileDb` extends [`hir::callgraph::WorkspaceFile`], so any feature
/// crate that takes `&dyn FileDb` automatically also gets
/// `content_hash()` and `callgraph()` for free — no extra trait bound
/// at the call site, no per-feature dispatch glue.
///
/// New methods land here only when an actual feature needs them; this
/// trait is intentionally small.
pub trait FileDb: hir::callgraph::SourceFileInfo {
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

    /// This file's syntax tree.
    ///
    /// From the cached green node, so asking for it costs a
    /// `SyntaxNode::new_root` rather than a re-parse. Features used to call
    /// `syntax::parse_text(file.text())` themselves, which parses the file
    /// again every time — and the assists could not reach a tree at all,
    /// which is why all sixty-four of them splice strings.
    fn syntax(&self) -> Option<syntax::SyntaxNode> {
        None
    }

    /// The database and file this handle stands for, when it has one.
    ///
    /// `FileDb` is a per-file view that hides the database, which is why
    /// features grew semantic logic of their own instead of asking
    /// `hir::Semantics` — rust-analyzer has no such trait and passes
    /// `&Semantics` and a `FileId`. This is the bridge across: a feature
    /// that has a `&dyn FileDb` can reach the one semantic API, and each
    /// one that does is one step of the migration rather than a rewrite of
    /// every signature at once.
    ///
    /// `None` for the test fixtures that implement `FileDb` over plain
    /// text with no database behind them.
    fn file_text(&self) -> Option<(&dyn salsa::Database, base_db::FileText)> {
        None
    }

    /// Per-file callable bodies (Body arena). Returns `None` when
    /// the parse pipeline failed for this file.
    fn bodies(&self) -> Option<&hir::CallableBodies> {
        None
    }

    fn body_scopes(&self, callable: &hir::CallableBody) -> hir::ExprScopes {
        if let Some((db, file)) = self.file_text() {
            return hir::source_order::bindings::body_scopes(
                db,
                file,
                &callable.body,
                callable.body_span.start,
            );
        }
        hir::ExprScopes::new_with_globals(&callable.body, |name| {
            self.item_tree().is_some_and(|tree| {
                tree.top_level_items().iter().any(|item| {
                    (item.name(tree) == name
                        && matches!(
                            item.def_kind(tree),
                            hir::DefKind::Let
                                | hir::DefKind::Register
                                | hir::DefKind::ValSpec
                                | hir::DefKind::Function
                                | hir::DefKind::Mapping
                                | hir::DefKind::Overload
                        ))
                        || (matches!(
                            item.def_kind(tree),
                            hir::DefKind::Enum | hir::DefKind::Union | hir::DefKind::Newtype
                        ) && item.fields(tree).iter().any(|field| &field.name == name))
                })
            })
        })
    }
}

/// Check a candidate declaration through the shared semantic scope. Plain
/// fixtures without a project remain in the global namespace.
pub fn symbol_visible_from(from: &dyn FileDb, target: &dyn FileDb, name: &str) -> bool {
    match (from.file_text(), target.file_text()) {
        (Some((db, from)), Some((_, target))) => {
            hir::visibility::symbol_visible_from(db, from.file_id(db), target, name)
        }
        _ => true,
    }
}
