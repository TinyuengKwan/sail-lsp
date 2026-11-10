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
        // Whitespace normalization runs once, over whitespace tokens only,
        // after the visitor. Line-based trimming here corrupts block comments.
        self.buffer.push_str(missing);
        self.last_pos = end;
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
