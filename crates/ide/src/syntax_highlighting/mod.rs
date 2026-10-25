mod types;
pub use types::*;
mod highlight;
pub mod html;
mod tags;

use ide_db::line_index::TextRange;
use ide_db::FileDb;

/// The token type legend (indices into this array are used in `HlRange::token_type`).
///
/// Extended for Sail-specific categories: register, enumMember, property,
/// struct, comment, builtinType.
pub const TOKEN_TYPES: &[&str] = &[
    "keyword",       // 0
    "function",      // 1
    "type",          // 2
    "enum",          // 3
    "variable",      // 4
    "typeParameter", // 5
    "string",        // 6
    "number",        // 7
    "operator",      // 8
    "struct",        // 9
    "enumMember",    // 10
    "property",      // 11
    "comment",       // 12
    "macro",         // 13 — reused for register (distinct color)
];

/// The modifier legend (bit indices into `HlRange::token_modifiers_bitset`).
///
/// Extended for Sail-specific modifiers.
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",   // bit 0
    "definition",    // bit 1
    "readonly",      // bit 2
    "modification",  // bit 3
    "documentation", // bit 4
    "static",        // bit 5 — for registers
    "controlFlow",   // bit 6
];

pub fn compute_semantic_tokens(file: &dyn FileDb) -> HlRanges {
    highlight::compute_semantic_tokens_filtered(file, None)
}

pub fn compute_semantic_tokens_range(file: &dyn FileDb, range: &TextRange) -> HlRanges {
    highlight::compute_semantic_tokens_filtered(file, Some(range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::{Span, Token};
    use tags::token_type_index;

    /// Minimal in-test stand-in for `sail_server::state::File` that
    /// implements [`FileDb`]. Keeps just enough state for the
    /// semantic-tokens tests: source text, lexed tokens, and a
    /// simple line-offset table for position mapping.
    struct TestFile {
        text: String,
        tokens: Vec<(Token, Span)>,
        /// Byte offsets where each line starts (0 for line 0).
        line_starts: Vec<usize>,
    }

    impl TestFile {
        fn new(source: &str) -> Self {
            let tokens = parser::tokenize(source);
            let mut line_starts = vec![0usize];
            for (i, ch) in source.char_indices() {
                if ch == '\n' {
                    line_starts.push(i + 1);
                }
            }
            Self { text: source.to_string(), tokens, line_starts }
        }

        fn line_for_offset(&self, offset: usize) -> u32 {
            // partition_point gives the first line whose start > offset
            let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
            line as u32
        }
    }

    impl hir::callgraph::WorkspaceFile for TestFile {
        fn content_hash(&self) -> u64 {
            0
        }
        fn callgraph(&self) -> Option<&hir::callgraph::CallGraph> {
            None
        }
    }

    impl hir::callgraph::SourceFileInfo for TestFile {
        fn text(&self) -> &str {
            &self.text
        }
        fn item_tree(&self) -> Option<&hir::ItemTree> {
            None
        }
    }

    impl FileDb for TestFile {
        fn position_at(&self, offset: usize) -> ide_db::LineCol {
            let line = self.line_for_offset(offset);
            let col = (offset - self.line_starts[line as usize]) as u32;
            ide_db::LineCol { line, col }
        }
        fn offset_at(&self, position: &ide_db::LineCol) -> usize {
            let line = (position.line as usize).min(self.line_starts.len() - 1);
            (self.line_starts[line] + position.col as usize).min(self.text.len())
        }
        fn tokens(&self) -> Option<&[(Token, Span)]> {
            Some(&self.tokens)
        }
        fn token_at(&self, _position: ide_db::LineCol) -> Option<&(Token, Span)> {
            None
        }
        fn parsed(&self) -> Option<&syntax::parser_lower::ParsedFile> {
            None
        }
    }

    #[test]
    fn highlight_as_html_with_spans() {
        use base_db::TextSize;
        let source = "val x : int";
        let r = base_db::TextRange::new(TextSize::from(0u32), TextSize::from(3u32));
        let highlights = vec![(r, "keyword")];
        let html = html::render_html(source, &highlights);
        assert!(html.contains("<span class=\"keyword\">val</span>"));
        assert!(html.contains("x : int"));
    }

    #[test]
    fn emits_non_empty_tokens() {
        let file = TestFile::new("function f(x) = x + 1");
        let tokens = compute_semantic_tokens(&file);
        assert!(!tokens.data.is_empty());
    }

    #[test]
    fn includes_keyword_and_number_tokens() {
        let file = TestFile::new("let x = 42");
        let tokens = compute_semantic_tokens(&file);
        assert!(tokens.data.iter().any(|t| t.token_type == 0));
        assert!(tokens.data.iter().any(|t| t.token_type == 7));
    }

    #[test]
    fn range_tokens_are_subset_of_full() {
        let file = TestFile::new("let x = 1\nlet y = 2\n");
        let full = compute_semantic_tokens(&file);
        let range = compute_semantic_tokens_range(&file, &base_db::text_range(0, 10));
        assert!(range.data.len() < full.data.len());
    }

    /// A name is classed by what it names, not as a variable because no
    /// declaring keyword precedes it.
    #[test]
    fn a_use_is_classed_by_its_definition() {
        let source = "enum E = { A, B }\nregister R : E\nfunction g() -> E = A\nfunction f() -> E = { R = g(); R }\n";
        let (analysis, files) = crate::fixture::analysis(&format!("//- /main.sail\n{source}"));
        let tokens = analysis.semantic_tokens(files[0].0).unwrap();
        let (mut line, mut col) = (0u32, 0u32);
        let mut at = std::collections::HashMap::new();
        for t in &tokens.data {
            if t.delta_line > 0 {
                line += t.delta_line;
                col = t.delta_start;
            } else {
                col += t.delta_start;
            }
            at.insert((line, col), t.token_type);
        }
        let type_of = |line: u32, needle: &str| {
            let col = source.lines().nth(line as usize).unwrap().rfind(needle).unwrap() as u32;
            at.get(&(line, col)).copied()
        };
        assert_eq!(type_of(2, "A"), Some(10), "enum member");
        assert_eq!(type_of(3, "g"), Some(1), "function");
        assert_eq!(type_of(3, "R"), Some(13), "register");
        assert_eq!(type_of(3, "E"), Some(3), "enum");
        // And a declaration is classed the same way as its uses.
        assert_eq!(type_of(0, "E"), Some(3), "enum declaration");
        assert_eq!(type_of(0, "B"), Some(10), "enum member declaration");
        assert_eq!(type_of(1, "R"), Some(13), "register declaration");
        assert_eq!(type_of(2, "g"), Some(1), "function declaration");
    }

    #[test]
    fn classifies_keywords_without_debug_string_hack() {
        assert_eq!(token_type_index(&parser::Token::KwFunction, None), Some(0));
    }
}
