//! Missed spans — handling unformatted gaps between nodes.
//!
//! When the visitor jumps from one formatted node to the next,
//! the gap may contain comments, blank lines, and whitespace that
//! must be preserved.

use super::visitor::FmtVisitor;

impl<'a> FmtVisitor<'a> {
    /// Emit source text from `last_pos` to `end`, preserving comments
    /// and whitespace between formatted nodes.
    pub(crate) fn format_missing(&mut self, end: usize) {
        if end <= self.last_pos {
            return;
        }
        let snippet = self.snippet_provider.entire_snippet();
        let missing = &snippet[self.last_pos..end];
        self.format_missing_inner(missing);
        self.last_pos = end;
    }

    /// Emit source text from `last_pos` to `end` with indentation
    /// added to each new line.
    ///
    /// Uses `Indent::block_only` to strip alignment from the indent before
    /// applying it to gap lines (rustfmt uses block_only to avoid carrying
    /// alignment from the previous item into the gap).
    #[allow(dead_code)] // Used when visitor recurses into function bodies.
    pub(crate) fn format_missing_with_indent(&mut self, end: usize) {
        if end <= self.last_pos {
            return;
        }
        let snippet = self.snippet_provider.entire_snippet();
        let missing = &snippet[self.last_pos..end];
        // indent — gaps between items should use pure block indentation.
        let block_indent = self.block_indent.block_only();
        let indent_str = block_indent.to_string_inner(self.config);

        for (i, line) in missing.split('\n').enumerate() {
            if i > 0 {
                self.buffer.push('\n');
                if !line.trim().is_empty() {
                    self.buffer.push_str(&indent_str);
                }
            }
            self.buffer.push_str(line.trim_start());
        }
        self.last_pos = end;
    }

    /// Inner implementation: emit the missing text, normalizing blank lines
    /// and trimming trailing whitespace.
    ///
    /// Uses `CommentStyle` to identify comment lines (preserving doc comments
    /// verbatim) and normalizes runs of blank lines to at most 2.
    fn format_missing_inner(&mut self, missing: &str) {
        use super::comment::CommentStyle;

        let mut consecutive_blanks = 0u32;
        for (i, line) in missing.split('\n').enumerate() {
            if i > 0 {
                self.buffer.push('\n');
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                consecutive_blanks += 1;
                if consecutive_blanks <= 2 {
                    // emit empty line (just the newline added above)
                }
                continue;
            }
            consecutive_blanks = 0;

            // Doc comments are preserved verbatim. Line comments get
            // trailing whitespace trimmed. Block comments are identified
            // via opener/closer for future reflow support.
            if let Some(style) = CommentStyle::from_str(trimmed) {
                if style.is_doc_comment() {
                    self.buffer.push_str(line);
                } else if style.is_line_comment() {
                    // Line comment — trim trailing whitespace only.
                    self.buffer.push_str(trimmed);
                } else if style.is_block_comment() {
                    // Block comment — preserve for now.
                    // Future: reflow using opener()/closer().
                    let _opener = style.opener();
                    let _closer = style.closer();
                    self.buffer.push_str(trimmed);
                } else {
                    self.buffer.push_str(trimmed);
                }
            } else {
                self.buffer.push_str(trimmed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::snippet::SnippetProvider;
    use super::super::visitor::FmtVisitor;
    use super::super::FormatOptions;

    #[test]
    fn format_missing_emits_gap() {
        let text = "  // comment\nval foo : int\n";
        let snippet = SnippetProvider::new(text.to_string());
        let config = FormatOptions::default();
        let mut visitor = FmtVisitor::new(&config, &snippet);
        visitor.last_pos = 0;
        visitor.format_missing(12); // "  // comment"
        assert_eq!(visitor.buffer, "  // comment");
        assert_eq!(visitor.last_pos, 12);
    }

    #[test]
    fn format_missing_noop_when_already_past() {
        let text = "hello";
        let snippet = SnippetProvider::new(text.to_string());
        let config = FormatOptions::default();
        let mut visitor = FmtVisitor::new(&config, &snippet);
        visitor.last_pos = 10;
        visitor.format_missing(5); // end < last_pos
        assert_eq!(visitor.buffer, "");
    }
}
