//! In-memory document tracking for open files.
//!
//! Sail-lsp: simplified to `HashMap<Url, DocumentData>`.

use lsp_types::Url;
use std::collections::HashMap;

/// Data for a single open document.
#[derive(Clone, Debug)]
pub(crate) struct DocumentData {
    pub version: i32,
    pub data: String,
}

impl DocumentData {
    /// Apply the client's incremental changes to this document.
    ///
    /// Lives here rather than in the handler because it is the one place
    /// that decides what the buffer now says, and a handler converts —
    /// it does not decide. Testable on its own, which is how the
    /// surrogate-pair case below is held.
    ///
    /// Each range is resolved against the text *as it stands*, because
    /// LSP specifies changes applied in order, each to the result of the
    /// last.
    pub fn apply_changes(&mut self, changes: &[lsp_types::TextDocumentContentChangeEvent]) {
        for change in changes {
            let Some(range) = &change.range else {
                self.data = change.text.clone();
                continue;
            };
            // Through `from_proto::offset`, which is where a client
            // position becomes a byte offset. This used to be a local
            // scan counting *characters* — a third reading of
            // `Position::character`, after the correct one (UTF-16 code
            // units) and the one this repo already fixed (bytes). They
            // agree across the whole Basic Multilingual Plane and part
            // company on a surrogate pair, so an emoji earlier on the
            // line shifted every later edit and silently corrupted the
            // buffer.
            //
            // `protocol_check` cannot see it: that probe exercises
            // `from_proto`/`to_proto`, and this path went round both.
            let index = ide_db::line_index::LineIndex::new(&self.data);
            let start = crate::lsp::from_proto::offset(&index, range.start);
            let end = crate::lsp::from_proto::offset(&index, range.end);
            if start <= end && end <= self.data.len() {
                self.data.replace_range(start..end, &change.text);
            } else {
                // A range this document cannot have. Editing at a guessed
                // offset is worse than declining: the buffer would
                // silently stop matching the client's.
                stdx::never!(
                    false,
                    "content change {start}..{end} outside a {}-byte document",
                    self.data.len()
                );
            }
        }
    }

    pub fn new(version: i32, data: String) -> Self {
        Self { version, data }
    }
}

/// Tracks all documents currently open in the editor.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemDocs {
    docs: HashMap<Url, DocumentData>,
    /// document set changed since last `take_changes()` call.
    added_or_removed: bool,
}

impl MemDocs {
    pub fn contains(&self, url: &Url) -> bool {
        self.docs.contains_key(url)
    }

    pub fn insert(&mut self, url: Url, data: DocumentData) {
        self.added_or_removed = true;
        self.docs.insert(url, data);
    }

    pub fn remove(&mut self, url: &Url) {
        self.added_or_removed = true;
        self.docs.remove(url);
    }

    /// Returns true if documents were added or removed since last call.
    pub fn take_changes(&mut self) -> bool {
        std::mem::take(&mut self.added_or_removed)
    }

    pub fn get(&self, url: &Url) -> Option<&DocumentData> {
        self.docs.get(url)
    }

    pub fn get_mut(&mut self, url: &Url) -> Option<&mut DocumentData> {
        self.docs.get_mut(url)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Url> {
        self.docs.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentData;
    use lsp_types::{Position, Range, TextDocumentContentChangeEvent};

    fn change(range: Range, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(range),
            range_length: None,
            text: text.to_string(),
        }
    }

    fn at(line: u32, s: u32, e: u32) -> Range {
        Range::new(Position::new(line, s), Position::new(line, e))
    }

    /// An edit lands where the client meant, past a character that is two
    /// UTF-16 units.
    ///
    /// `Position::character` counts UTF-16 code units. A `🦀` is one
    /// `char` and *two* of them, so a scan that counted characters put
    /// every later edit on that line one unit early — and the buffer then
    /// silently disagrees with the editor's, which is the worst kind of
    /// wrong: everything downstream is right about the wrong text.
    #[test]
    fn an_edit_past_a_surrogate_pair_lands_where_the_client_meant() {
        let mut doc = DocumentData::new(1, "// 🦀 abc\n".to_string());
        // `abc` starts at UTF-16 column 6: `/`(0) `/`(1) ` `(2) 🦀(3,4)
        // ` `(5). Counting *characters* puts it at 5, and the edit then
        // lands one place early — which is what this used to do.
        doc.apply_changes(&[change(at(0, 6, 9), "xyz")]);
        assert_eq!(doc.data, "// 🦀 xyz\n");
    }

    /// The same edit on an all-ASCII line, where every encoding agrees.
    #[test]
    fn an_ascii_edit_is_unaffected() {
        let mut doc = DocumentData::new(1, "// cr abc\n".to_string());
        doc.apply_changes(&[change(at(0, 6, 9), "xyz")]);
        assert_eq!(doc.data, "// cr xyz\n");
    }

    /// Changes apply in order, each to the result of the last.
    #[test]
    fn changes_apply_in_order() {
        let mut doc = DocumentData::new(1, "abc\n".to_string());
        doc.apply_changes(&[change(at(0, 0, 1), "X"), change(at(0, 1, 2), "Y")]);
        assert_eq!(doc.data, "XYc\n");
    }

    /// A whole-document change replaces it.
    #[test]
    fn a_change_without_a_range_replaces_the_document() {
        let mut doc = DocumentData::new(1, "old\n".to_string());
        doc.apply_changes(&[TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "new\n".to_string(),
        }]);
        assert_eq!(doc.data, "new\n");
    }
    #[test]
    fn invalid_columns_stay_on_their_line_and_surrogates_do_not_split() {
        let mut doc = DocumentData::new(1, "é🦀\r\n中文".into());
        doc.apply_changes(&[change(at(0, u32::MAX, u32::MAX), "!")]);
        assert_eq!(doc.data, "é🦀!\r\n中文");
        doc.apply_changes(&[change(at(0, 2, 3), "X")]);
        assert_eq!(doc.data, "éX!\r\n中文");
        doc.apply_changes(&[change(at(99, 0, 0), "end")]);
        assert_eq!(doc.data, "éX!\r\n中文end");
    }

    #[test]
    fn randomized_unicode_change_batches_match_independent_byte_edits() {
        fn position(text: &str, offset: usize) -> Position {
            let mut p = Position::new(0, 0);
            let mut chars = text.char_indices().peekable();
            while let Some((i, c)) = chars.next() {
                if i >= offset {
                    break;
                }
                if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    continue;
                }
                if c == '\r' || c == '\n' {
                    p = Position::new(p.line + 1, 0);
                } else {
                    p.character += c.len_utf16() as u32;
                }
            }
            p
        }
        let mut seed = 29u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize
        };
        let pieces = ["", "a", "é", "🦀", "中文", "\r", "\n", "\r\n", "é\n🦀"];
        let mut reference = "中文🦀\r\nline\nend".to_string();
        let mut doc = DocumentData::new(1, reference.clone());
        for round in 0..500 {
            let mut changes = Vec::new();
            if round % 37 == 0 {
                reference = "🦀 reset\r\n中文".into();
                changes.push(TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: reference.clone(),
                });
            }
            for _ in 0..4 {
                let boundaries: Vec<_> = reference
                    .char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(reference.len()))
                    .filter(|&i| {
                        !(i > 0
                            && reference.as_bytes().get(i) == Some(&b'\n')
                            && reference.as_bytes()[i - 1] == b'\r')
                    })
                    .collect();
                let a = boundaries[next() % boundaries.len()];
                let b = boundaries[next() % boundaries.len()];
                let (a, b) = (a.min(b), a.max(b));
                let replacement = pieces[next() % pieces.len()];
                changes.push(change(
                    Range::new(position(&reference, a), position(&reference, b)),
                    replacement,
                ));
                reference.replace_range(a..b, replacement);
            }
            doc.apply_changes(&changes);
            assert_eq!(doc.data, reference, "batch {round}");
        }
    }
}
