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
/// A character that is more than one byte, and more than one UTF-16 unit.
///
/// rust-analyzer's `WideChar` (`lib/line-index`), and for both of its
/// reasons: a client column is a UTF-16 column and has to be converted, and
/// a byte offset that is not a character boundary cannot index a `&str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct WideChar {
    /// Byte offset from the start of its line.
    col: usize,
    /// Length in bytes.
    bytes: usize,
    /// Length in UTF-16 code units.
    utf16: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineIndex {
    /// Byte offset of the start of each line. `line_starts[0] == 0`.
    line_starts: Vec<usize>,
    /// End of each line, excluding its line terminator.
    line_ends: Vec<usize>,
    /// Total length of the text in bytes.
    len: usize,
    /// Per line, the characters wider than one byte, in column order. Empty
    /// for the overwhelmingly common all-ASCII line, so the cost is a
    /// `Vec::is_empty` on that path.
    wide: Vec<Vec<WideChar>>,
}

impl LineIndex {
    /// Build a `LineIndex` from source text.
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0usize];
        let mut line_ends = Vec::new();
        let mut wide: Vec<Vec<WideChar>> = vec![Vec::new()];
        for (i, ch) in text.char_indices() {
            if ch == '\r' && text.as_bytes().get(i + 1) == Some(&b'\n') {
                continue;
            }
            if ch == '\n' || ch == '\r' {
                line_ends.push(if ch == '\n' && i > 0 && text.as_bytes()[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                });
                line_starts.push(i + 1);
                wide.push(Vec::new());
                continue;
            }
            if ch.len_utf8() > 1 {
                let line_start = *line_starts.last().expect("line_starts is never empty");
                let last = wide.last_mut().expect("wide has one entry per line start");
                last.push(WideChar {
                    col: i - line_start,
                    bytes: ch.len_utf8(),
                    utf16: ch.len_utf16(),
                });
            }
        }
        line_ends.push(text.len());
        Self { line_starts, line_ends, len: text.len(), wide }
    }

    /// The byte column a UTF-16 column names, on the given line.
    ///
    /// LSP's default `positionEncoding` is UTF-16 and this server had no
    /// conversion at all: `from_proto` passed `Position::character` straight
    /// through as a byte column. On a line holding any non-ASCII character
    /// — the bundled prelude's own licence header has several — every
    /// position from the client was wrong, and slicing at one panicked.
    pub fn utf16_col_to_byte(&self, line: LineNr, utf16_col: ColNr) -> ColNr {
        let Some(chars) = self.wide.get(line as usize) else { return 0 };
        let line_len = self.line_ends[line as usize] - self.line_starts[line as usize];
        let utf16_col = (utf16_col as usize).min(line_len) as ColNr;
        if chars.is_empty() {
            return utf16_col;
        }
        let mut byte = 0usize;
        let mut wide = 0usize;
        for wc in chars {
            // The ASCII run before this character.
            let ascii = wc.col - byte;
            if utf16_col as usize <= wide + ascii {
                // `byte` and `wide` are the columns immediately after the
                // previous non-ASCII character. Once such a character has
                // been crossed they differ, so returning `utf16_col`
                // unchanged is only correct before the first wide character
                // on a line. This is the same cumulative adjustment made by
                // rust-analyzer's `LineIndex::to_utf8`.
                return (byte + (utf16_col as usize - wide)) as ColNr;
            }
            byte = wc.col + wc.bytes;
            wide += ascii + wc.utf16;
            if (utf16_col as usize) < wide {
                // Inside the wide character: its own start is the answer.
                return wc.col as ColNr;
            }
        }
        (byte + (utf16_col as usize - wide)).min(line_len) as ColNr
    }

    /// Convert a byte offset to a line/col position.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let col = offset - self.line_starts[line];
        LineCol { line: line as LineNr, col: col as ColNr }
    }

    /// Convert a line/col position to a byte offset.
    /// The UTF-16 column a byte column names, on the given line.
    ///
    /// The other half of `utf16_col_to_byte`: what goes *out* is a
    /// `Position::character`, which LSP reads as UTF-16. Sending a byte
    /// column put every diagnostic, hint and navigation range on a
    /// non-ASCII line at the wrong place.
    pub fn byte_col_to_utf16(&self, line: LineNr, byte_col: ColNr) -> ColNr {
        let Some(chars) = self.wide.get(line as usize) else { return 0 };
        let col = (byte_col as usize)
            .min(self.line_ends[line as usize] - self.line_starts[line as usize]);
        let mut adjustment = 0;
        for wc in chars {
            if col <= wc.col {
                break;
            }
            if col < wc.col + wc.bytes {
                return (wc.col - adjustment) as ColNr;
            }
            adjustment += wc.bytes - wc.utf16;
        }
        (col - adjustment) as ColNr
    }

    /// The byte offset a `LineCol` names, always on a character boundary.
    ///
    /// A client column is not a byte column. LSP's default `positionEncoding`
    /// is UTF-16, and this adds `col` to the line start unchanged, so on a
    /// line holding any non-ASCII character the offset lands *inside* it —
    /// `// Union's Horizon 2020` in the bundled prelude's own licence header
    /// is such a line — and every `&text[..offset]` on the way to an answer
    /// panics. `never!` records that and snaps down to the boundary, because
    /// an answer at a slightly wrong column beats killing the request:
    /// "use them when terminating on assertion failure is worse than
    /// continuing" (`stdx::assert`, vendored from `always-assert`, and this
    /// is its first caller).
    ///
    /// Snapping is containment, not the fix. The fix is to agree with the
    /// client on an encoding, which `positionEncoding` negotiation does.
    pub fn offset(&self, pos: LineCol) -> usize {
        let line = pos.line as usize;
        let Some(&line_start) = self.line_starts.get(line) else { return self.len };
        let raw = (line_start + pos.col as usize).min(self.line_ends[line]);
        // Inside a wide character? Only if the caller handed us a column in
        // an encoding this line does not use.
        let inside = self.wide.get(line).is_some_and(|chars| {
            chars.iter().any(|wc| {
                let start = line_start + wc.col;
                raw > start && raw < start + wc.bytes
            })
        });
        if stdx::never!(inside, "line/col {}:{} is inside a character", pos.line, pos.col) {
            let chars = &self.wide[line];
            let wc = chars
                .iter()
                .find(|wc| {
                    let start = line_start + wc.col;
                    raw > start && raw < start + wc.bytes
                })
                .expect("`inside` is true only when one matches");
            return line_start + wc.col;
        }
        raw
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

#[cfg(test)]
mod utf16_tests {
    use super::*;

    /// The bundled prelude's own licence header, which is why this matters.
    const WIDE: &str = "// Union\u{2019}s Horizon 2020\nval x : int\n";

    /// A byte column and a UTF-16 column convert to each other.
    ///
    /// LSP's default `positionEncoding` is UTF-16 and this server negotiates
    /// nothing else; positions used to be passed through as byte columns, so
    /// on any line with a non-ASCII character every one of them was wrong.
    #[test]
    fn a_column_round_trips_through_utf16() {
        let idx = LineIndex::new(WIDE);
        let line0 = WIDE.lines().next().expect("a first line");
        for (byte_col, _) in line0.char_indices() {
            let utf16 = idx.byte_col_to_utf16(0, byte_col as ColNr);
            assert_eq!(
                idx.utf16_col_to_byte(0, utf16),
                byte_col as ColNr,
                "byte {byte_col} -> utf16 {utf16} -> back"
            );
        }
    }

    #[test]
    fn a_column_between_two_wide_characters_is_adjusted() {
        let text = "étarget🦀\n";
        let idx = LineIndex::new(text);

        assert_eq!(idx.utf16_col_to_byte(0, 1), 2);
        assert_eq!(idx.utf16_col_to_byte(0, 4), 5);
    }

    /// An all-ASCII line is the identity, which is every line in practice.
    #[test]
    fn an_ascii_column_is_unchanged() {
        let idx = LineIndex::new(WIDE);
        for col in 0..11u32 {
            assert_eq!(idx.byte_col_to_utf16(1, col), col);
            assert_eq!(idx.utf16_col_to_byte(1, col), col);
        }
    }

    /// Every UTF-16 column a client can send yields an offset that can
    /// index the text.
    ///
    /// This is the path the server takes — `from_proto::offset` converts,
    /// then asks for the offset — and it is what keeps the `never!` inside
    /// `offset` from ever firing. `never!` panics under `debug_assertions`
    /// by design, so a test that reached it would be asserting the wrong
    /// thing: the guarantee is that the conversion makes it unreachable.
    #[test]
    fn every_client_column_indexes_the_text() {
        let idx = LineIndex::new(WIDE);
        let line0 = WIDE.lines().next().expect("a first line");
        let utf16_len = line0.chars().map(char::len_utf16).sum::<usize>() as ColNr;
        for utf16_col in 0..=utf16_len + 2 {
            let byte_col = idx.utf16_col_to_byte(0, utf16_col);
            let off = idx.offset(LineCol { line: 0, col: byte_col });
            assert!(
                WIDE.is_char_boundary(off),
                "utf16 column {utf16_col} gave byte offset {off}, not a character boundary"
            );
        }
    }
}
