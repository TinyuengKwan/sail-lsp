//! A test file backed by a real salsa database.
//!
//! `TestFile` used to run its own lex → parse → lower pipeline and cache the
//! results in plain fields. That made every test using it check a pipeline
//! the server does not run: derived data came from different code, and
//! `ref_counts`/`impl_counts` were simply left empty, so any feature reading
//! them saw zeroes in tests and real numbers in production.
//!
//! It is now the same salsa database and the same queries `SalsaFile` uses,
//! with the workspace registered as the server registers it. Call sites are
//! unchanged.
//!
//! ```ignore
//! use ide_db::test_utils::TestFile;
//! let file = TestFile::new("function f(x) = x + 1\n");
//! ```

use std::sync::Arc;

use base_db::{FileId, FileText};
use hir::callgraph::{CallGraph, WorkspaceFile};
use parser::{Span, Token};
use syntax::parser_lower::ParsedFile;

use crate::line_index::LineIndex;
use crate::root_database::RootDatabase;
use crate::{FileDb, LineCol};

/// A single-file workspace in a real salsa database.
#[derive(Clone)]
pub struct TestFile {
    db: RootDatabase,
    input: FileText,
    line_index: LineIndex,
}

impl TestFile {
    /// Build a test file from source text.
    pub fn new(source: &str) -> Self {
        let mut db = RootDatabase::default();
        let text: Arc<str> = Arc::from(source);
        let input = FileText::new(&db, text, FileId::from_raw(0));
        // Register the workspace the way the server does on load. Without it
        // the cross-file queries have no context and `infix` declarations
        // never reach the parser.
        hir::def_query::set_workspace_files(&mut db, vec![input]);
        Self { db, input, line_index: LineIndex::new(source) }
    }

    /// The salsa database behind this file, for tests that query it directly.
    pub fn db(&self) -> &RootDatabase {
        &self.db
    }

    /// The file's salsa input.
    pub fn input(&self) -> FileText {
        self.input
    }
}

impl WorkspaceFile for TestFile {
    fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.input.text(&self.db).hash(&mut hasher);
        hasher.finish()
    }

    fn callgraph(&self) -> Option<&CallGraph> {
        hir::def_query::callgraph(&self.db, self.input).as_ref().map(|acg| acg.0.as_ref())
    }
}

impl hir::callgraph::SourceFileInfo for TestFile {
    fn text(&self) -> &str {
        self.input.text(&self.db).as_ref()
    }

    fn item_tree(&self) -> Option<&hir::ItemTree> {
        hir::def_query::file_item_tree(&self.db, self.input).as_deref()
    }
}

impl FileDb for TestFile {
    fn file_text(&self) -> Option<(&dyn salsa::Database, FileText)> {
        Some((&self.db, self.input))
    }

    fn syntax(&self) -> Option<syntax::SyntaxNode> {
        let parsed = syntax::parse_query::parse_file(&self.db, self.input);
        parsed.green.as_ref().map(|g| syntax::SyntaxNode::new_root((**g).clone()))
    }

    // text() and item_tree() come from SourceFileInfo.

    fn position_at(&self, offset: usize) -> LineCol {
        self.line_index.line_col(offset)
    }

    fn offset_at(&self, position: &LineCol) -> usize {
        self.line_index.offset(*position)
    }

    fn tokens(&self) -> Option<&[(Token, Span)]> {
        let parsed = syntax::parse_query::parse_file(&self.db, self.input);
        if parsed.tokens.is_empty() {
            None
        } else {
            Some(parsed.tokens.as_slice())
        }
    }

    fn token_at(&self, position: LineCol) -> Option<&(Token, Span)> {
        let offset = self.offset_at(&position);
        let tokens = self.tokens()?;
        // The token containing the offset, or — if none does — the one that
        // ends there. A cursor immediately after a name is where an editor
        // leaves it once the name is typed, and where a double-click then a
        // goto often lands; a half-open test alone answered nothing at all
        // for it. The containing token still wins, so no existing answer
        // changes.
        tokens
            .iter()
            .rev()
            .find(|(_, span)| span.start <= offset && offset < span.end)
            .or_else(|| tokens.iter().rev().find(|(_, span)| span.end == offset))
    }

    fn parsed(&self) -> Option<&ParsedFile> {
        crate::db_query::enriched_parsed_file(&self.db, self.input).as_ref().map(Arc::as_ref)
    }

    fn bodies(&self) -> Option<&hir::CallableBodies> {
        hir::def_query::callable_bodies(&self.db, self.input).as_ref().map(|b| b.0.as_ref())
    }
}
