//! Source change — multi-file edit representation.

use std::collections::HashMap;

use base_db::FileId;

use crate::text_edit::TextEdit;

/// A set of edits across multiple files.
///
/// ```text
/// pub struct SourceChange {
///     pub source_file_edits: IntMap<FileId, (TextEdit, Option<SnippetEdit>)>,
///     pub file_system_edits: Vec<FileSystemEdit>,
///     pub is_snippet: bool,
/// }
/// ```
///
/// Simplified for Sail (no snippet edits, no annotations).
#[derive(Clone, Debug, Default)]
pub struct SourceChange {
    /// Per-file text edits.
    pub source_file_edits: HashMap<FileId, Vec<TextEdit>>,
    /// File system operations (create, move, delete).
    pub file_system_edits: Vec<FileSystemEdit>,
    /// Whether the edits contain snippet placeholders.
    pub is_snippet: bool,
}

impl SourceChange {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert edits for a single file.
    pub fn insert_source_edit(&mut self, file_id: FileId, edits: Vec<TextEdit>) {
        self.source_file_edits.entry(file_id).or_default().extend(edits);
    }

    pub fn is_empty(&self) -> bool {
        self.source_file_edits.is_empty() && self.file_system_edits.is_empty()
    }
}

impl SourceChange {
    /// Construct a `SourceChange` from a single-file edit.
    pub fn from_file_edit(file_id: FileId, edit: TextEdit) -> Self {
        let mut change = Self::new();
        change.insert_source_edit(file_id, vec![edit]);
        change
    }

    /// Construct a `SourceChange` from a text edit without a file ID.
    ///
    /// Used by diagnostic fixes where the file context is implicit.
    /// The actual file will be resolved when converting to LSP.
    pub fn from_text_edit(edit: TextEdit) -> Self {
        // Use FileId::from_raw(0) as placeholder — the LSP layer will resolve
        // the correct file from the diagnostic's URI context.
        let mut change = Self::new();
        change.insert_source_edit(FileId::from_raw(0), vec![edit]);
        change
    }
}

/// Builder for constructing a `SourceChange` for a single file.
/// Collects edits and produces a `SourceChange` on `finish()`.
pub struct SourceChangeBuilder {
    file_id: FileId,
    edits: Vec<TextEdit>,
    source_change: SourceChange,
}

impl SourceChangeBuilder {
    /// Create a builder targeting a single file.
    pub fn new(file_id: FileId) -> Self {
        Self { file_id, edits: Vec::new(), source_change: SourceChange::new() }
    }

    /// Insert text at an offset.
    pub fn insert(&mut self, offset: usize, text: impl Into<String>) {
        self.edits
            .push(TextEdit { range: base_db::text_range(offset, offset), new_text: text.into() });
    }

    /// Delete a range of text.
    pub fn delete(&mut self, range: crate::line_index::TextRange) {
        self.edits.push(TextEdit { range, new_text: String::new() });
    }

    /// Replace a range of text.
    pub fn replace(&mut self, range: crate::line_index::TextRange, text: impl Into<String>) {
        self.edits.push(TextEdit { range, new_text: text.into() });
    }

    /// Finish building and produce the `SourceChange`.
    pub fn finish(mut self) -> SourceChange {
        if !self.edits.is_empty() {
            self.source_change.insert_source_edit(self.file_id, self.edits);
        }
        self.source_change
    }
}

/// A file system operation.
#[derive(Clone, Debug)]
pub enum FileSystemEdit {
    /// Create a new file.
    CreateFile {
        /// Anchor file (the new file is created relative to this).
        anchor: FileId,
        /// Destination path (relative to anchor).
        dst: String,
    },
    /// Move/rename a file.
    MoveFile { src: FileId, dst: String },
}
