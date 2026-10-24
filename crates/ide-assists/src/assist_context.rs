//! Assist context and accumulator.
//! `AssistContext` holds file + range + cached tokens;
//! `Assists` accumulates results via closure-based editing.
//!
//! Types are re-exported from `ide-db::assists` — single source of truth.

use crate::assist_config::AssistConfig;
use ide_db::line_index::TextRange;
use ide_db::source_change::SourceChangeBuilder;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

// Re-export types from ide-db for use in handlers.
pub use ide_db::assists::{Assist, AssistId, AssistKind, GroupLabel, Label};

/// What an assist is given to work with.
///
/// It used to be a file handle and a range, and nothing else — no syntax
/// tree, no semantics, no config. That is why all sixty-four handlers
/// build their edits by splicing strings and re-derive the syntax they
/// are editing: the context offered nothing else to use.
/// rust-analyzer's holds `config`, `sema`, `source_file` and a cached
/// `token_at_offset`, and its handlers edit the tree.
pub struct AssistContext<'a> {
    /// How the client wants assists computed — which kinds are allowed,
    /// whether snippets are supported, how to fill a missing body.
    pub config: &'a AssistConfig,
    /// File being analyzed.
    pub file: &'a dyn FileDb,
    /// Target range (cursor position or selection).
    pub range: TextRange,
    /// The file's syntax tree, parsed once for the whole assist run.
    source_file: Option<syntax::SyntaxNode>,
}

impl<'a> AssistContext<'a> {
    pub fn new(config: &'a AssistConfig, file: &'a dyn FileDb, range: TextRange) -> Self {
        // `FileDb::syntax()` defaults to `None`, so a handle that did not
        // override it left every handler without a tree — which is why
        // eight of them each parsed the text again. Parsed once, here.
        let source_file = file.syntax().or_else(|| Some(syntax::parse_text(file.text()).0));
        Self { config, file, range, source_file }
    }

    /// The file's syntax tree.
    pub fn source_file(&self) -> Option<&syntax::SyntaxNode> {
        self.source_file.as_ref()
    }

    /// Semantic queries about this file, when it is backed by a database.
    pub fn sema(&self) -> Option<(hir::Semantics<'_>, &dyn salsa::Database, base_db::FileText)> {
        let (db, ft) = self.file.file_text()?;
        Some((hir::Semantics::new(db), db, ft))
    }

    /// The innermost node covering the target range.
    pub fn covering_element(&self) -> Option<syntax::SyntaxNode> {
        let root = self.source_file()?;
        let start = base_db::range_start(self.range);
        let end = base_db::range_end(self.range);
        root.descendants()
            .filter(|n| {
                let r = n.text_range();
                usize::from(r.start()) <= start && end <= usize::from(r.end())
            })
            .min_by_key(|n| n.text_range().len())
    }

    /// Cursor offset (start of range).
    pub fn offset(&self) -> usize {
        base_db::range_start(self.range)
    }

    /// Whether the selection is empty (cursor, not selection).
    pub fn has_empty_selection(&self) -> bool {
        base_db::range_start(self.range) == base_db::range_end(self.range)
    }

    /// Trimmed selection range (whitespace stripped from edges).
    pub fn selection_trimmed(&self) -> TextRange {
        if self.has_empty_selection() {
            return self.range;
        }
        let text = self.file.text();
        let start = base_db::range_start(self.range);
        let end = base_db::range_end(self.range);
        let selected = &text[start..end.min(text.len())];
        let trimmed = selected.trim();
        if trimmed.is_empty() {
            return self.range;
        }
        let trim_start = start + (selected.len() - selected.trim_start().len());
        let trim_end = end - (selected.len() - selected.trim_end().len());
        base_db::text_range(trim_start, trim_end)
    }

    /// Get the token at the cursor offset.
    pub fn token_at_offset(&self) -> Option<&(parser::Token, parser::Span)> {
        let offset = self.offset();
        let tokens = self.file.tokens()?;
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

    // There is no `find_token_text_at_offset`. It sliced the source at
    // the token's span; a `Token` carries its own text for every kind
    // that has one, and the handlers reach `token_at_offset` and match on
    // it. Slicing the file to recover what the token already holds is the
    // shape this workspace keeps deleting.

    /// Source text of the file.
    pub fn source_text(&self) -> &str {
        self.file.text()
    }
}

/// Accumulator for assists being computed.
/// RA supports: `add()` with closure-based editing, `add_group()`,
/// resolve strategy filtering, and sorting.
pub struct Assists {
    buf: Vec<Assist>,
    file_id: base_db::FileId,
}

impl Default for Assists {
    fn default() -> Self {
        Self::new(base_db::FileId::from_raw(0))
    }
}

impl Assists {
    /// The file being edited, so an assist can name it — and so a future
    /// one can name another. `SourceChangeBuilder::new(FileId::from_raw(0))`
    /// was hardcoded here, which made the multi-file change `SourceChange`
    /// exists for impossible even on the path that produced one.
    pub fn new(file_id: base_db::FileId) -> Self {
        Self { buf: Vec::new(), file_id }
    }

    /// Add an assist with closure-based editing.
    ///
    /// ```ignore
    /// acc.add(id, label, target, |builder| { builder.replace(...); })
    /// ```
    ///
    /// The closure receives a `SourceChangeBuilder` and produces edits.
    pub fn add(
        &mut self,
        id: AssistId,
        label: impl Into<String>,
        target: TextRange,
        f: impl FnOnce(&mut SourceChangeBuilder),
    ) -> Option<()> {
        let label = label.into();
        let mut builder = SourceChangeBuilder::new(self.file_id);
        f(&mut builder);
        let source_change = builder.finish();
        if source_change.is_empty() {
            return None;
        }
        self.buf.push(Assist {
            import: None,
            id,
            label: Label::new(label),
            group: None,
            target,
            source_change: Some(source_change),
            command: None,
        });
        Some(())
    }

    /// Add an assist from edits the handler already computed.
    ///
    /// The same `SourceChange` as `add`, just built from a list rather than
    /// through the builder — which is the difference RA's `add` hides
    /// behind its closure.
    pub fn add_with_edits(
        &mut self,
        id: AssistId,
        label: impl Into<String>,
        target: TextRange,
        edits: Vec<TextEdit>,
    ) {
        if edits.is_empty() {
            return;
        }
        let mut source_change = ide_db::source_change::SourceChange::new();
        source_change.insert_source_edit(self.file_id, edits);
        self.buf.push(Assist {
            import: None,
            id,
            label: Label::new(label.into()),
            group: None,
            target,
            source_change: Some(source_change),
            command: None,
        });
    }

    /// Add an assist with a group label.
    pub fn add_group(
        &mut self,
        group: &GroupLabel,
        id: AssistId,
        label: impl Into<String>,
        target: TextRange,
        f: impl FnOnce(&mut SourceChangeBuilder),
    ) -> Option<()> {
        let label = label.into();
        let mut builder = SourceChangeBuilder::new(self.file_id);
        f(&mut builder);
        let source_change = builder.finish();
        if source_change.is_empty() {
            return None;
        }
        self.buf.push(Assist {
            import: None,
            id,
            label: Label::new(label),
            group: Some(group.clone()),
            target,
            source_change: Some(source_change),
            command: None,
        });
        Some(())
    }

    /// Preserve the candidate so a lazy code action can revalidate its edit.
    pub fn add_import(
        &mut self,
        candidate: ide_db::imports::import_assets::LocatedImport,
        target: TextRange,
        edit: TextEdit,
    ) {
        self.add_with_edits(
            AssistId("auto_include", AssistKind::QuickFix),
            format!("Add `$include \"{}\"`", candidate.include_path),
            target,
            vec![edit],
        );
        self.buf.last_mut().unwrap().import = Some(candidate.request);
    }

    /// Finish and return all collected assists.
    pub fn finish(self) -> Vec<Assist> {
        self.buf
    }
}

/// Handler function type.
///
/// `type Handler = fn(&mut Assists, &AssistContext<'_>) -> Option<()>`
pub type Handler = fn(&mut Assists, &AssistContext<'_>) -> Option<()>;
