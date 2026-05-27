mod highlight;
pub mod html;
mod tags;

use ide_db::ide_types::{HlRange, HlRanges, HlRangesDelta, HlRangesEdit};
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

fn semantic_tokens_result_id(file: &dyn FileDb, token_count: usize) -> String {
    format!("{:x}:{:x}", file.text().len(), token_count)
}

pub fn compute_semantic_tokens(file: &dyn FileDb) -> HlRanges {
    highlight::compute_semantic_tokens_filtered(file, None)
}

pub fn compute_semantic_tokens_range(file: &dyn FileDb, range: &TextRange) -> HlRanges {
    highlight::compute_semantic_tokens_filtered(file, Some(range))
}

pub fn compute_semantic_tokens_delta(previous: &HlRanges, current: &HlRanges) -> HlRangesDelta {
    let previous_data = &previous.data;
    let current_data = &current.data;

    let mut prefix = 0_usize;
    let prefix_limit = previous_data.len().min(current_data.len());
    while prefix < prefix_limit && previous_data[prefix] == current_data[prefix] {
        prefix += 1;
    }

    let mut suffix = 0_usize;
    while suffix + prefix < previous_data.len()
        && suffix + prefix < current_data.len()
        && previous_data[previous_data.len() - 1 - suffix]
            == current_data[current_data.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let edits = if prefix == previous_data.len() && prefix == current_data.len() {
        Vec::new()
    } else {
        vec![HlRangesEdit {
            start: prefix as u32,
            delete_count: (previous_data.len() - prefix - suffix) as u32,
            data: Some(current_data[prefix..(current_data.len() - suffix)].to_vec()),
        }]
    };

    HlRangesDelta { result_id: current.result_id.clone(), edits }
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

    impl hir_def::callgraph::WorkspaceFile for TestFile {
        fn content_hash(&self) -> u64 {
            0
        }
        fn callgraph(&self) -> Option<&hir_def::callgraph::CallGraph> {
            None
        }
    }

    impl hir_def::callgraph::SourceFileInfo for TestFile {
        fn text(&self) -> &str {
            &self.text
        }
        fn item_tree(&self) -> Option<&hir_def::ItemTree> {
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
        fn signature_index(
            &self,
        ) -> Option<&std::collections::HashMap<String, ide_db::CallableSignature>> {
            None
        }
        fn ref_counts(&self) -> &std::collections::HashMap<String, usize> {
            static EMPTY: std::sync::OnceLock<std::collections::HashMap<String, usize>> =
                std::sync::OnceLock::new();
            EMPTY.get_or_init(std::collections::HashMap::new)
        }
        fn impl_counts(&self) -> &std::collections::HashMap<String, usize> {
            static EMPTY: std::sync::OnceLock<std::collections::HashMap<String, usize>> =
                std::sync::OnceLock::new();
            EMPTY.get_or_init(std::collections::HashMap::new)
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

    #[test]
    fn classifies_keywords_without_debug_string_hack() {
        assert_eq!(token_type_index(&parser::Token::KwFunction, None), Some(0));
    }

    #[test]
    fn semantic_token_delta_uses_incremental_edit() {
        let old_file = TestFile::new("let x = 1");
        let new_file = TestFile::new("let xyz = 100");
        let previous = compute_semantic_tokens(&old_file);
        let current = compute_semantic_tokens(&new_file);
        let delta = compute_semantic_tokens_delta(&previous, &current);
        assert!(!delta.edits.is_empty());
    }
}
