//! Source change — multi-file edit representation.

use std::collections::HashMap;

use base_db::FileId;

use crate::text_edit::TextEdit;

/// A set of edits across multiple files.
///
/// ```text
/// pub struct SourceChange {
///     pub source_file_edits: IntMap<FileId, (TextEdit, Option<SnippetEdit>)>,
///     pub is_snippet: bool,
/// }
/// ```
///
/// File creation is represented separately so clients can apply the creation
/// and the edits to existing sources as one workspace operation.
#[derive(Clone, Debug, Default)]
pub struct SourceChange {
    pub file_creations: Vec<FileCreation>,
    /// Per-file text edits.
    pub source_file_edits: HashMap<FileId, Vec<TextEdit>>,
    /// Whether any edit's text is a snippet — `$0` where the cursor
    /// should land, `${1:name}` for something to type over. A client that
    /// cannot expand those gets them stripped.
    pub is_snippet: bool,
}

#[derive(Clone, Debug)]
pub struct FileCreation {
    pub anchor: FileId,
    pub path: String,
    pub contents: String,
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
        self.source_file_edits.is_empty() && self.file_creations.is_empty()
    }
}

impl SourceChange {
    /// Construct a `SourceChange` from a single-file edit.
    /// No caller, and kept.
    ///
    /// Every quick fix today edits the file its diagnostic is in, so the
    /// fourteen call sites use `from_text_edit` and let the conversion
    /// supply the file. This is the constructor a fix that edits a
    /// *different* file needs — `SourceChange` exists to carry several —
    /// and deleting it would leave the wrong one as the only one. Wired
    /// when the first such fix arrives, not before.
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
    /// Create a sibling file, anchored at the source being edited.
    pub fn create_file(&mut self, path: String, contents: String) {
        self.source_change.file_creations.push(FileCreation {
            anchor: self.file_id,
            path,
            contents,
        });
    }

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

    /// Insert a snippet at an offset — text that may carry `$0` for
    /// where the cursor should land, or `${1:name}` for something to type
    /// over.
    ///
    /// This is rust-analyzer's `insert_snippet`, and the reason its
    /// generate-* assists leave you where you have to type next. Nothing
    /// here set `SourceChange::is_snippet`, so nothing could.
    pub fn insert_snippet(&mut self, offset: usize, snippet: impl Into<String>) {
        self.source_change.is_snippet = true;
        self.insert(offset, snippet);
    }

    /// Replace a range with a snippet. RA's `replace_snippet`.
    pub fn replace_snippet(
        &mut self,
        range: crate::line_index::TextRange,
        snippet: impl Into<String>,
    ) {
        self.source_change.is_snippet = true;
        self.replace(range, snippet);
    }

    /// Finish building and produce the `SourceChange`.
    pub fn finish(mut self) -> SourceChange {
        if !self.edits.is_empty() {
            self.source_change.insert_source_edit(self.file_id, self.edits);
        }
        self.source_change
    }
}

impl SourceChangeBuilder {
    /// An editor over the tree `node` belongs to.
    ///
    /// rust-analyzer's assists edit the tree and let this turn the result
    /// into text; sixty-four handlers here formatted strings and computed
    /// offsets by hand, while `SyntaxEditor`, `SyntaxFactory`, `ast::make`
    /// and `ted` — 2,676 lines between them — had no caller outside the
    /// `syntax` crate. This is the bridge that was missing.
    pub fn make_editor(&self, node: &syntax::SyntaxNode) -> syntax::syntax_editor::SyntaxEditor {
        syntax::syntax_editor::SyntaxEditor::new(
            node.ancestors().last().unwrap_or_else(|| node.clone()),
        )
        .0
    }

    /// Apply an editor's changes as a text edit.
    ///
    /// The narrowest replacement that turns the old text into the new one:
    /// the trees differ somewhere in the middle, and sending the whole file
    /// back would move every cursor and fold in the editor.
    pub fn add_file_edits(&mut self, editor: syntax::syntax_editor::SyntaxEditor) {
        let edit = editor.finish();
        let old = edit.old_root().to_string();
        let new = edit.new_root().to_string();
        if old == new {
            return;
        }
        let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(max_suffix);
        // Byte offsets have to land on character boundaries.
        let mut start = prefix;
        while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
            start -= 1;
        }
        let mut old_end = old.len() - suffix;
        let mut new_end = new.len() - suffix;
        while !old.is_char_boundary(old_end) || !new.is_char_boundary(new_end) {
            old_end += 1;
            new_end += 1;
        }
        self.replace(base_db::text_range(start, old_end), new[start..new_end].to_owned());
    }
}
