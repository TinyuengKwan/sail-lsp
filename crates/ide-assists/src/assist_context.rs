//! Assist context and accumulator.
//! `AssistContext` holds file + range + cached tokens;
//! `Assists` accumulates results via closure-based editing.
//!
//! Types are re-exported from `ide-db::assists` — single source of truth.

use ide_db::line_index::TextRange;
use ide_db::source_change::SourceChangeBuilder;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

// Re-export types from ide-db for use in handlers.
pub use ide_db::assists::{Assist, AssistId, AssistKind, GroupLabel, Label};

/// Context for assist computation.
/// RA holds: `config`, `sema`, `frange`, `source_file`,
///   cached `token_at_offset`, cached `covering_element`.
/// We hold: `file`, `range`, cached `token_at_offset`.
pub struct AssistContext<'a> {
    /// File being analyzed.
    pub file: &'a dyn FileDb,
    /// Target range (cursor position or selection).
    ///
    /// FileId is implicit in `file`).
    pub range: TextRange,
}

impl<'a> AssistContext<'a> {
    pub fn new(file: &'a dyn FileDb, range: TextRange) -> Self {
        Self { file, range }
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
        tokens.iter().rev().find(|(_, span)| span.start <= offset && offset < span.end)
    }

    /// Find a token's text and span at the cursor offset.
    pub fn find_token_text_at_offset(&self) -> Option<(&str, parser::Span)> {
        let (_, span) = self.token_at_offset()?;
        let text = self.file.text();
        if span.end <= text.len() {
            Some((&text[span.start..span.end], *span))
        } else {
            None
        }
    }

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
}

impl Assists {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
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
        let mut builder = SourceChangeBuilder::new(base_db::FileId::from_raw(0));
        f(&mut builder);
        let source_change = builder.finish();
        if source_change.is_empty() {
            return None;
        }
        self.buf.push(Assist {
            id,
            label: Label::new(label),
            group: None,
            target,
            source_change: Some(source_change),
            command: None,
            edits: Vec::new(),
        });
        Some(())
    }

    /// Add an assist with pre-computed text edits.
    ///
    /// Sail bridge: handlers that produce TextEdit directly (legacy path).
    /// Will be migrated to closure-based `add()` over time.
    pub fn add_with_edits(
        &mut self,
        id: AssistId,
        label: impl Into<String>,
        target: TextRange,
        edits: Vec<TextEdit>,
    ) {
        if !edits.is_empty() {
            self.buf.push(Assist {
                id,
                label: Label::new(label.into()),
                group: None,
                target,
                source_change: None,
                command: None,
                edits,
            });
        }
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
        let mut builder = SourceChangeBuilder::new(base_db::FileId::from_raw(0));
        f(&mut builder);
        let source_change = builder.finish();
        if source_change.is_empty() {
            return None;
        }
        self.buf.push(Assist {
            id,
            label: Label::new(label),
            group: Some(group.clone()),
            target,
            source_change: Some(source_change),
            command: None,
            edits: Vec::new(),
        });
        Some(())
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
