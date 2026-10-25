use crate::line_index::LineCol;

/// An incremental text change: replace a range (or the entire document).
///
/// Framework-independent replacement for `lsp_types::TextDocumentContentChangeEvent`.
pub struct TextChange {
    /// The range to replace, or `None` for a full-document replacement.
    pub range: Option<TextChangeRange>,
    /// The new text.
    pub text: String,
}

/// A line/col range for an incremental text change.
pub struct TextChangeRange {
    pub start: LineCol,
    pub end: LineCol,
}

#[derive(Clone)]
pub struct TextDocument {
    // The text.
    content: String,
    // The start of each line in bytes. These could be lazily calculated but
    // that is a bit tricky because of the borrow checker.
    line_offsets: Vec<usize>,
}

impl TextDocument {
    pub fn new(content: String) -> Self {
        let line_offsets = compute_line_offsets(&content, 0);
        Self { content, line_offsets }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    #[cfg(test)]
    pub fn line_count(&self) -> usize {
        self.line_offsets.len()
    }

    // Based on this code https://github.com/microsoft/vscode-languageserver-node/blob/master/textDocument/src/main.ts#L222
    // Apply a change to the document.
    pub fn update(&mut self, change: &TextChange) {
        if let Some(range) = &change.range {
            // Get the corresponding byte range.
            let byte_begin = self.offset_at(&range.start);
            let byte_end = self.offset_at(&range.end);
            if byte_begin > byte_end {
                return;
            }
            // Include the preceding line: inserting LF at a line start
            // can merge it with a preceding CR. Scanning through the end
            // line's terminator covers the other splice boundary.
            let first_line = (self.position_at(byte_begin).line as usize).saturating_sub(1);
            let past_line =
                (self.position_at(byte_end).line as usize + 1).min(self.line_offsets.len());
            let scan_start = self.line_offsets[first_line];
            let scan_end = self.line_start(past_line);
            let has_tail = past_line < self.line_offsets.len();
            self.content.replace_range(byte_begin..byte_end, &change.text);
            let new_scan_end = byte_begin + change.text.len() + (scan_end - byte_end);
            let mut offsets =
                compute_line_offsets(&self.content[scan_start..new_scan_end], scan_start);
            if has_tail && offsets.last() == Some(&new_scan_end) {
                offsets.pop();
            }
            for offset in &mut self.line_offsets[past_line..] {
                *offset = byte_begin + change.text.len() + (*offset - byte_end);
            }
            self.line_offsets.splice(first_line..past_line, offsets);
        } else {
            // Just completely change the text.
            self.content = change.text.clone();
            self.line_offsets = compute_line_offsets(&self.content, 0);
        }
    }

    // Convert a row/column position to a byte index.
    pub fn offset_at(&self, position: &LineCol) -> usize {
        let line_begin = self.line_start(position.line as usize);
        let line_end = self.line_start(position.line as usize + 1);
        let line = self.content[line_begin..line_end].trim_end_matches(['\r', '\n']);

        line_begin + character_to_line_offset(line, position.col as usize)
    }

    // Convert a byte index to a row/column position.
    pub fn position_at(&self, offset: usize) -> LineCol {
        // Clamp to valid range.
        let mut offset = std::cmp::min(offset, self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }

        // Binary search for a line offset that is greater than the offset.
        match self.line_offsets.binary_search(&offset) {
            Ok(line) => {
                // offset actually matches one of the line_offsets. Just return it.
                LineCol { line: line as u32, col: 0 }
            }
            Err(line) => {
                // line is the line one past the one that contains the offset.
                let line = line - 1;
                let character = self.position_at_line(line, offset);

                LineCol { line: line as u32, col: character as u32 }
            }
        }
    }

    fn line_start(&self, line_index: usize) -> usize {
        self.line_offsets.get(line_index).copied().unwrap_or(self.content.len())
    }

    // Given a byte offset, what is the corresponding character?
    fn position_at_line(&self, line: usize, offset: usize) -> usize {
        let line_start = self.line_offsets[line];

        assert!(line_start <= offset);

        // We have to scan through the line, counting the characters.
        let line_text = &self.content[line_start..offset];
        line_text.chars().map(char::len_utf16).sum()
    }
}

// Given a UTF-16 codepoint offset in a bit of text, convert it to a byte offset.
// Out-of-bounds offsets just return line.len().
fn character_to_line_offset(line: &str, character: usize) -> usize {
    let mut utf16_pos = 0;

    for (byte_pos, ch) in line.char_indices() {
        if utf16_pos + ch.len_utf16() > character {
            return byte_pos;
        }
        utf16_pos += ch.len_utf16();
    }

    line.len()
}

fn compute_line_offsets(text: &str, text_offset: usize) -> Vec<usize> {
    // VSCode treats `\r\n` or `\n` *or* `\r` as a single line end.
    let mut line_offsets = vec![text_offset];
    let text = text.as_bytes();
    for i in 0..text.len() {
        match text[i] {
            b'\n' => line_offsets.push(text_offset + i + 1),
            b'\r'
                // This is a new line *unless* the next character is \n.
                if text.get(i + 1) != Some(&b'\n') => {
                    line_offsets.push(text_offset + i + 1)
                }
            _ => {}
        }
    }
    line_offsets
}

#[cfg(test)]
mod test {

    use super::*;

    fn lc(line: u32, col: u32) -> LineCol {
        LineCol { line, col }
    }

    fn text_range(doc: &TextDocument, start: &LineCol, end: &LineCol) -> String {
        let byte_begin = doc.offset_at(start);
        let byte_end = doc.offset_at(end);
        doc.text()[byte_begin..byte_end].to_string()
    }

    #[test]
    fn empty_content() {
        let text = "".to_string();
        let document = TextDocument::new(text);
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.offset_at(&lc(0, 0)), 0);
        assert_eq!(document.position_at(0), lc(0, 0));
    }

    #[test]
    fn single_line() {
        let text = "Hello World".to_string();
        let document = TextDocument::new(text.clone());
        assert_eq!(document.line_count(), 1);

        for (char_index, (byte_index, _)) in text.char_indices().enumerate() {
            assert_eq!(document.offset_at(&lc(0, char_index as u32)), byte_index);
            assert_eq!(document.position_at(byte_index), lc(0, char_index as u32));
        }
    }

    #[test]
    fn multiple_lines() {
        let text = "ABCDE\nFGHIJ\nKLMNO\n".to_string();
        let document = TextDocument::new(text.clone());
        assert_eq!(document.line_count(), 4);

        for (char_index, (byte_index, _)) in text.char_indices().enumerate() {
            let line = char_index / 6;
            let column = char_index % 6;

            assert_eq!(document.offset_at(&lc(line as u32, column as u32)), byte_index);
            assert_eq!(document.position_at(byte_index), lc(line as u32, column as u32));
        }

        assert_eq!(document.offset_at(&lc(3, 0)), 18);
        assert_eq!(document.offset_at(&lc(3, 1)), 18);
        assert_eq!(document.position_at(18), lc(3, 0));
        assert_eq!(document.position_at(19), lc(3, 0));
    }

    #[test]
    fn starts_with_new_line() {
        let document = TextDocument::new("\nABCDE".to_string());
        assert_eq!(document.line_count(), 2);
        assert_eq!(document.position_at(0), lc(0, 0));
        assert_eq!(document.position_at(1), lc(1, 0));
        assert_eq!(document.position_at(6), lc(1, 5));
    }

    #[test]
    fn new_line_characters() {
        let text = "ABCDE\rFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 2);

        let text = "ABCDE\nFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 2);

        let text = "ABCDE\r\nFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 2);

        let text = "ABCDE\n\nFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 3);

        let text = "ABCDE\r\rFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 3);

        let text = "ABCDE\n\rFGHIJ".to_string();
        assert_eq!(TextDocument::new(text).line_count(), 3);
    }

    #[test]
    fn get_text_range() {
        let text = "12345\n12345\n12345".to_string();
        let document = TextDocument::new(text.clone());
        assert_eq!(document.text(), text);
        assert_eq!(text_range(&document, &lc(0, 0), &lc(0, 5)), "12345");
        assert_eq!(text_range(&document, &lc(0, 4), &lc(1, 1)), "5\n1");
        assert_eq!(text_range(&document, &lc(0, 4), &lc(2, 1)), "5\n12345\n1");
        assert_eq!(text_range(&document, &lc(0, 4), &lc(3, 1)), "5\n12345\n12345");
        assert_eq!(text_range(&document, &lc(0, 0), &lc(3, 5)), text);
    }

    #[test]
    fn invalid_inputs() {
        let text = "Hello World".to_string();
        let document = TextDocument::new(text.clone());

        // invalid position
        assert_eq!(document.offset_at(&lc(0, text.len() as u32)), text.len());
        assert_eq!(document.offset_at(&lc(0, text.len() as u32 + 3)), text.len());
        assert_eq!(document.offset_at(&lc(2, 3)), text.len());

        // invalid offsets
        assert_eq!(document.position_at(text.len()), lc(0, text.len() as u32));
        assert_eq!(document.position_at(text.len() + 3), lc(0, text.len() as u32));
    }

    // Full updates.

    #[test]
    fn one_full_update() {
        let mut document = TextDocument::new("abc123".to_string());
        document.update(&TextChange { text: "efg456".to_string(), range: None });
        assert_eq!(document.text(), "efg456");
    }

    #[test]
    fn several_full_content_updates() {
        let mut document = TextDocument::new("abc123".to_string());
        document.update(&TextChange { text: "hello".to_string(), range: None });
        document.update(&TextChange { text: "world".to_string(), range: None });
        assert_eq!(document.text(), "world");
    }

    // Incremental updates.

    // assumes that only "\n" is used
    fn assert_valid_line_numbers(doc: &TextDocument) {
        let text = doc.text();
        let mut expected_line_number = 0;
        for (i, ch) in doc.text().char_indices() {
            assert_eq!(doc.position_at(i).line, expected_line_number);
            if ch == '\n' {
                expected_line_number += 1;
            }
        }
        assert_eq!(doc.position_at(text.len()).line, expected_line_number);
    }

    fn range_for_substring(doc: &TextDocument, needle: &str) -> TextChangeRange {
        let offset = doc.text().find(needle).unwrap();
        TextChangeRange {
            start: doc.position_at(offset),
            end: doc.position_at(offset + needle.len()),
        }
    }

    fn range_after_substring(doc: &TextDocument, needle: &str) -> TextChangeRange {
        let offset = doc.text().find(needle).unwrap();
        let pos = doc.position_at(offset + needle.len());
        TextChangeRange { start: pos, end: pos }
    }

    #[test]
    fn incrementally_removing_content() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "".to_string(),
            range: Some(range_for_substring(&document, "hello, world!")),
        });
        assert_eq!(document.text(), "function abc() {\n  console.log(\"\");\n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_removing_multi_line_content() {
        let mut document =
            TextDocument::new("function abc() {\n  foo();\n  bar();\n  \n}".to_string());
        assert_eq!(document.line_count(), 5);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "".to_string(),
            range: Some(range_for_substring(&document, "  foo();\n  bar();\n")),
        });
        assert_eq!(document.text(), "function abc() {\n  \n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_removing_multi_line_content_2() {
        let mut document =
            TextDocument::new("function abc() {\n  foo();\n  bar();\n  \n}".to_string());
        assert_eq!(document.line_count(), 5);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "".to_string(),
            range: Some(range_for_substring(&document, "foo();\n  bar();")),
        });
        assert_eq!(document.text(), "function abc() {\n  \n  \n}");
        assert_eq!(document.line_count(), 4);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_adding_content() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: ", world!".to_string(),
            range: Some(range_after_substring(&document, "hello")),
        });
        assert_eq!(document.text(), "function abc() {\n  console.log(\"hello, world!\");\n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_adding_multi_line_content() {
        let mut document = TextDocument::new(
            "function abc() {\n  while (true) {\n    foo();\n  };\n}".to_string(),
        );
        assert_eq!(document.line_count(), 5);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "\n    bar();".to_string(),
            range: Some(range_after_substring(&document, "foo();")),
        });
        assert_eq!(
            document.text(),
            "function abc() {\n  while (true) {\n    foo();\n    bar();\n  };\n}"
        );
        assert_eq!(document.line_count(), 6);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_single_line_content_more_chars() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "hello, test case!!!".to_string(),
            range: Some(range_for_substring(&document, "hello, world!")),
        });
        assert_eq!(document.text(), "function abc() {\n  console.log(\"hello, test case!!!\");\n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_single_line_content_less_chars() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "hey".to_string(),
            range: Some(range_for_substring(&document, "hello, world!")),
        });
        assert_eq!(document.text(), "function abc() {\n  console.log(\"hey\");\n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_single_line_content_same_num_of_chars() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "world, hello!".to_string(),
            range: Some(range_for_substring(&document, "hello, world!")),
        });
        assert_eq!(document.text(), "function abc() {\n  console.log(\"world, hello!\");\n}");
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_multi_line_content_more_lines() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        assert_eq!(document.line_count(), 3);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "\n//hello\nfunction d(){".to_string(),
            range: Some(range_for_substring(&document, "function abc() {")),
        });
        assert_eq!(
            document.text(),
            "\n//hello\nfunction d(){\n  console.log(\"hello, world!\");\n}"
        );
        assert_eq!(document.line_count(), 5);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_multi_line_content_less_lines() {
        let mut document = TextDocument::new("a1\nb1\na2\nb2\na3\nb3\na4\nb4\n".to_string());
        assert_eq!(document.line_count(), 9);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "xx\nyy".to_string(),
            range: Some(range_for_substring(&document, "\na3\nb3\na4\nb4\n")),
        });
        assert_eq!(document.text(), "a1\nb1\na2\nb2xx\nyy");
        assert_eq!(document.line_count(), 5);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_multi_line_content_same_num_of_lines_and_chars() {
        let mut document = TextDocument::new("a1\nb1\na2\nb2\na3\nb3\na4\nb4\n".to_string());
        assert_eq!(document.line_count(), 9);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "\nxx1\nxx2".to_string(),
            range: Some(range_for_substring(&document, "a2\nb2\na3")),
        });
        assert_eq!(document.text(), "a1\nb1\n\nxx1\nxx2\nb3\na4\nb4\n");
        assert_eq!(document.line_count(), 9);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn incrementally_replacing_multi_line_content_same_num_of_lines_but_diff_chars() {
        let mut document = TextDocument::new("a1\nb1\na2\nb2\na3\nb3\na4\nb4\n".to_string());
        assert_eq!(document.line_count(), 9);
        assert_valid_line_numbers(&document);
        document.update(&TextChange {
            text: "\ny\n".to_string(),
            range: Some(range_for_substring(&document, "a2\nb2\na3")),
        });
        assert_eq!(document.text(), "a1\nb1\n\ny\n\nb3\na4\nb4\n");
        assert_eq!(document.line_count(), 9);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn several_incremental_content_changes() {
        let mut document =
            TextDocument::new("function abc() {\n  console.log(\"hello, world!\");\n}".to_string());
        document.update(&TextChange {
            text: "defg".to_string(),
            range: Some(TextChangeRange { start: lc(0, 12), end: lc(0, 12) }),
        });
        document.update(&TextChange {
            text: "hello, test case!!!".to_string(),
            range: Some(TextChangeRange { start: lc(1, 15), end: lc(1, 28) }),
        });
        document.update(&TextChange {
            text: "hij".to_string(),
            range: Some(TextChangeRange { start: lc(0, 16), end: lc(0, 16) }),
        });

        assert_eq!(
            document.text(),
            "function abcdefghij() {\n  console.log(\"hello, test case!!!\");\n}"
        );
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn basic_append() {
        let mut document = TextDocument::new("foooo\nbar\nbaz".to_string());

        assert_eq!(document.offset_at(&lc(2, 0)), 10);

        document.update(&TextChange {
            text: " some extra content".to_string(),
            range: Some(TextChangeRange { start: lc(1, 3), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foooo\nbar some extra content\nbaz");
        assert_eq!(document.offset_at(&lc(2, 0)), 29);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn multi_line_append() {
        let mut document = TextDocument::new("foooo\nbar\nbaz".to_string());

        assert_eq!(document.offset_at(&lc(2, 0)), 10);

        document.update(&TextChange {
            text: " some extra\ncontent".to_string(),
            range: Some(TextChangeRange { start: lc(1, 3), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foooo\nbar some extra\ncontent\nbaz");
        assert_eq!(document.offset_at(&lc(3, 0)), 29);
        assert_eq!(document.line_count(), 4);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn basic_delete() {
        let mut document = TextDocument::new("foooo\nbar\nbaz".to_string());

        assert_eq!(document.offset_at(&lc(2, 0)), 10);

        document.update(&TextChange {
            text: "".to_string(),
            range: Some(TextChangeRange { start: lc(1, 0), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foooo\n\nbaz");
        assert_eq!(document.offset_at(&lc(2, 0)), 7);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn multi_line_delete() {
        let mut document = TextDocument::new("foooo\nbar\nbaz".to_string());

        assert_eq!(document.offset_at(&lc(2, 0)), 10);

        document.update(&TextChange {
            text: "".to_string(),
            range: Some(TextChangeRange { start: lc(0, 5), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foooo\nbaz");
        assert_eq!(document.offset_at(&lc(1, 0)), 6);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn single_character_replace() {
        let mut document = TextDocument::new("foooo\nbar\nbaz".to_string());

        assert_eq!(document.offset_at(&lc(2, 0)), 10);

        document.update(&TextChange {
            text: "z".to_string(),
            range: Some(TextChangeRange { start: lc(1, 2), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foooo\nbaz\nbaz");
        assert_eq!(document.offset_at(&lc(2, 0)), 10);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn multi_character_replace() {
        let mut document = TextDocument::new("foo\nbar".to_string());

        assert_eq!(document.offset_at(&lc(1, 0)), 4);

        document.update(&TextChange {
            text: "foobar".to_string(),
            range: Some(TextChangeRange { start: lc(1, 0), end: lc(1, 3) }),
        });
        assert_eq!(document.text(), "foo\nfoobar");
        assert_eq!(document.offset_at(&lc(1, 0)), 4);
        assert_valid_line_numbers(&document);
    }

    #[test]
    fn unicode_positions_clamp_to_character_boundaries() {
        let doc = TextDocument::new("é🦀中\r\nx".into());
        assert_eq!(doc.offset_at(&lc(0, 2)), 2);
        assert_eq!(doc.offset_at(&lc(0, 99)), 9);
        assert_eq!(doc.position_at(4), lc(0, 1));
        assert_eq!(doc.offset_at(&lc(99, 99)), doc.text().len());
    }

    #[test]
    fn invalid_update_ranges_are_bounded_and_reversed_ranges_are_ignored() {
        let mut doc = TextDocument::new("foo\nbar".into());
        doc.update(&TextChange {
            range: Some(TextChangeRange { start: lc(3, 0), end: lc(6, 10) }),
            text: "中文".into(),
        });
        assert_eq!(doc.text(), "foo\nbar中文");
        doc.update(&TextChange {
            range: Some(TextChangeRange { start: lc(1, 2), end: lc(0, 0) }),
            text: "bad".into(),
        });
        assert_eq!(doc.text(), "foo\nbar中文");
        assert_valid_line_numbers(&doc);
    }

    #[test]
    fn edits_form_and_split_crlf_at_splice_boundaries() {
        let mut doc = TextDocument::new("a\rb".into());
        doc.update(&TextChange {
            range: Some(TextChangeRange { start: lc(1, 0), end: lc(1, 0) }),
            text: "\n".into(),
        });
        assert_eq!(doc.text(), "a\r\nb");
        assert_eq!(doc.line_offsets, vec![0, 3]);
        doc.update(&TextChange {
            range: Some(TextChangeRange { start: lc(0, 1), end: lc(1, 0) }),
            text: "\n\r".into(),
        });
        assert_eq!(doc.text(), "a\n\rb");
        assert_eq!(doc.line_offsets, vec![0, 2, 3]);
    }

    #[test]
    fn randomized_unicode_edits_match_a_fresh_document() {
        fn position(text: &str, offset: usize) -> LineCol {
            let mut p = lc(0, 0);
            let mut chars = text.char_indices().peekable();
            while let Some((i, c)) = chars.next() {
                if i >= offset {
                    break;
                }
                if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    continue;
                }
                if c == '\r' || c == '\n' {
                    p = lc(p.line + 1, 0);
                } else {
                    p.col += c.len_utf16() as u32;
                }
            }
            p
        }
        let mut seed = 17u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize
        };
        let pieces = ["", "x", "é", "🦀", "中文", "\n", "\r", "\r\n", "a\n🦀"];
        let mut reference = "é🦀中\r\nfirst\nlast".to_string();
        let mut doc = TextDocument::new(reference.clone());
        for _ in 0..2000 {
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
            doc.update(&TextChange {
                range: Some(TextChangeRange {
                    start: position(&reference, a),
                    end: position(&reference, b),
                }),
                text: replacement.into(),
            });
            reference.replace_range(a..b, replacement);
            assert_eq!(doc.text(), reference);
            assert_eq!(doc.line_offsets, TextDocument::new(reference.clone()).line_offsets);
            for (i, _) in reference.char_indices() {
                if i > 0 && reference.as_bytes()[i] == b'\n' && reference.as_bytes()[i - 1] == b'\r'
                {
                    continue;
                }
                let p = position(&reference, i);
                assert_eq!(doc.position_at(i), p);
                assert_eq!(doc.offset_at(&p), i);
            }
        }
    }
}
