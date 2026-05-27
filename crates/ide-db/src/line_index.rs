//! Line/column ↔ byte-offset index for a source file.
//!
//! Framework-independent replacement for the scattered `position_at` /
//! `offset_at` implementations. Mirrors rust-analyzer's `LineIndex`
//! from `crates/line-index`.
//!
//! Created in stage .

/// A 0-based line number.
pub type LineNr = u32;
/// A 0-based UTF-8 column offset within a line.
pub type ColNr = u32;

/// Line + column position in a text document (0-based).
///
/// Framework-independent equivalent of `lsp_types::Position`.
/// Conversion to/from LSP Position happens only at the handler boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineCol {
    /// 0-based line number.
    pub line: LineNr,
    /// 0-based UTF-8 column offset.
    pub col: ColNr,
}

/// Re-export TextRange from base-db so all existing `ide_db::line_index::TextRange`
/// paths continue to work. The canonical definition lives in base-db to avoid
/// circular dependencies (hir-ty needs TextRange but can't depend on ide-db).
pub use base_db::TextRange;

/// Pre-computed line-start table for fast line/col ↔ offset conversion.
///
/// Build once per file, reuse for all position queries.
/// `LineIndex` from `crates/line-index`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineIndex {
    /// Byte offset of the start of each line. `line_starts[0] == 0`.
    line_starts: Vec<usize>,
    /// Total length of the text in bytes.
    len: usize,
}

impl LineIndex {
    /// Build a `LineIndex` from source text.
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, ch) in text.char_indices() {
            if ch == '\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts, len: text.len() }
    }

    /// Convert a byte offset to a line/col position.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let col = offset - self.line_starts[line];
        LineCol { line: line as LineNr, col: col as ColNr }
    }

    /// Convert a line/col position to a byte offset.
    pub fn offset(&self, pos: LineCol) -> usize {
        let line = (pos.line as usize).min(self.line_starts.len() - 1);
        (self.line_starts[line] + pos.col as usize).min(self.len)
    }

    /// Number of lines in the text.
    pub fn num_lines(&self) -> usize {
        self.line_starts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_basic() {
        let text = "hello\nworld\n";
        let idx = LineIndex::new(text);

        assert_eq!(idx.num_lines(), 3); // "hello\n", "world\n", ""
        assert_eq!(idx.line_col(0), LineCol { line: 0, col: 0 });
        assert_eq!(idx.line_col(5), LineCol { line: 0, col: 5 }); // '\n'
        assert_eq!(idx.line_col(6), LineCol { line: 1, col: 0 }); // 'w'
        assert_eq!(idx.line_col(11), LineCol { line: 1, col: 5 }); // '\n'
    }

    #[test]
    fn line_index_roundtrip() {
        let text = "val x : int\nfunction foo(x) = x + 1\n";
        let idx = LineIndex::new(text);

        for offset in 0..text.len() {
            let lc = idx.line_col(offset);
            let back = idx.offset(lc);
            assert_eq!(back, offset, "roundtrip failed at offset {offset}");
        }
    }

    #[test]
    fn line_index_empty() {
        let idx = LineIndex::new("");
        assert_eq!(idx.num_lines(), 1);
        assert_eq!(idx.line_col(0), LineCol { line: 0, col: 0 });
    }

    #[test]
    fn text_range_from_span() {
        let span = parser::Span::new(10, 20);
        let range = base_db::span_to_text_range(&span);
        assert_eq!(base_db::range_start(range), 10);
        assert_eq!(base_db::range_end(range), 20);
        assert_eq!(base_db::range_len(range), 10);
    }
}
