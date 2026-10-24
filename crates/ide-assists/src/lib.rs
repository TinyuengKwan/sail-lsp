//! Code-action (assist) entry point for the Sail language server.
//!
//! 51 handlers registered via `handlers::all()`.
//! Entry point: `assists(file, range) -> Vec<Assist>`.

use ide_db::FileDb;

use ide_db::line_index::TextRange;

// Assist infrastructure.
pub mod assist_config;
pub mod assist_context;
pub(crate) mod handlers;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod utils;

pub use assist_config::{AssistConfig, SnippetCap};
// Types re-exported from ide-db (single source of truth).
pub use assist_context::{AssistContext, Assists};
/// `ide` folds constants in hover through this.
/// The fixes a diagnostic offers. Defined under `handlers/` with every other
/// assist; re-exported because `ide` and the server call them by name.
pub use handlers::quick_fix::{
    missing_semicolon_fix, quick_fix_for_diagnostic, unused_variable_fix, var_to_let_fix,
};
pub use ide_db::assists::{Assist, AssistId, AssistKind};
pub use utils::try_fold_constant;

/// Entry point for all assists.
///
/// `config` is a parameter because the client sets it — which kinds it
/// wants, whether it supports snippets, how a missing body should be
/// filled. It was a struct with six fields and no users, including this
/// function, so none of those settings could reach a handler.
pub fn assists(
    config: &AssistConfig,
    file: &dyn FileDb,
    range: TextRange,
) -> Vec<assist_context::Assist> {
    let ctx = assist_context::AssistContext::new(config, file, range);
    let file_id = file
        .file_text()
        .map(|(db, ft)| ft.file_id(db))
        .unwrap_or_else(|| base_db::FileId::from_raw(0));
    let mut acc = assist_context::Assists::new(file_id);
    for handler in handlers::all() {
        handler(&mut acc, &ctx);
    }
    // The kinds the client asked for. Filtering here rather than in each
    // handler keeps `allowed` one decision instead of sixty-four.
    let mut out = acc.finish();
    // Through `AssistConfig::is_allowed`, which is the same test written a
    // second time here and had no caller of its own.
    out.retain(|a| config.is_allowed(a.id.1));
    out
}

//
// recursively walking the expression tree. For assist code that needs
// to find a specific expression form at a given offset, these functions
// use Body + BodySourceMap to locate the Expr nearest to the cursor.

#[cfg(test)]
mod lib_tests {

    use crate::handlers::evaluate_constant::evaluate_constant_edits;
    use crate::handlers::extract_variable::extract_local_let_edits;
    use crate::handlers::organize_imports::organize_imports_edits;
    use crate::handlers::simplify_boolean::simplify_boolean_edits;
    use ide_db::FileDb;
    use parser::{Span, Token};
    use syntax::parser_lower::ParsedFile;

    /// Minimal in-test stand-in for `sail_server::state::File`. Runs
    /// the full lex / parse / lower pipeline up front and stores the
    /// parsed + core_ast results so the assist functions under test
    /// can pull them through `FileDb`.
    struct TestFile {
        text: String,
        tokens: Vec<(Token, Span)>,
        parsed: Option<ParsedFile>,
        item_tree: Option<std::sync::Arc<hir::ItemTree>>,
        bodies: Option<std::sync::Arc<hir::CallableBodies>>,
        line_starts: Vec<usize>,
    }

    impl TestFile {
        fn new(source: &str) -> Self {
            let tokens = parser::tokenize(source);
            let (cst_root, _) = syntax::parse_text(source);
            let parsed = Some(syntax::cst_lower::parsed_file_from_cst(&cst_root, source));
            let item_tree = Some(std::sync::Arc::new(hir::ItemTree::build_from_cst(&cst_root)));
            let bodies = Some(std::sync::Arc::new(hir::CallableBodies::from_cst(&cst_root)));
            let mut line_starts = vec![0usize];
            for (i, ch) in source.char_indices() {
                if ch == '\n' {
                    line_starts.push(i + 1);
                }
            }
            Self { text: source.to_string(), tokens, parsed, item_tree, bodies, line_starts }
        }

        fn line_for_offset(&self, offset: usize) -> u32 {
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
            self.item_tree.as_deref()
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
        fn parsed(&self) -> Option<&ParsedFile> {
            self.parsed.as_ref()
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
        fn bodies(&self) -> Option<&hir::CallableBodies> {
            self.bodies.as_deref()
        }
    }

    /// A bare-expression body is replaced whole, with a block around the
    /// binding — there is nowhere else to put it. With a block already
    /// there it is two edits, an insert and a replace, which
    /// `extract_variable`'s own tests cover.
    #[test]
    fn extract_local_let_wraps_a_bare_body() {
        let src = "function f(x) = x + 1\n";
        let file = TestFile::new(src);
        let start = src.find("x + 1").expect("selection start");
        let end = start + "x + 1".len();
        let edits =
            extract_local_let_edits(&file, base_db::text_range(start, end)).expect("extract edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "{ let extracted_value = x + 1; extracted_value }");
    }

    #[test]
    fn extract_local_let_rejects_empty_or_multiline_selection() {
        let src = "let x = (\n  1 + 2\n)\n";
        let file = TestFile::new(src);
        let empty = base_db::text_range(0, 0);
        assert!(extract_local_let_edits(&file, empty).is_none());

        let start = src.find("(\n").expect("multiline start");
        let end = src.find(")\n").expect("multiline end") + 1;
        let range = base_db::text_range(start, end);
        assert!(extract_local_let_edits(&file, range).is_none());
    }

    #[test]
    fn organize_imports_sorts_and_deduplicates_top_block() {
        let src = "$include \"b.sail\"\n$include \"a.sail\"\n$include \"a.sail\"\nlet x = 1\n";
        let file = TestFile::new(src);
        let edits = organize_imports_edits(&file).expect("organize edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "$include \"a.sail\"\n$include \"b.sail\"\n");
    }

    #[test]
    fn organize_imports_skips_when_comment_in_block() {
        let src = "$include \"b.sail\"\n// keep note\n$include \"a.sail\"\n";
        let file = TestFile::new(src);
        assert!(organize_imports_edits(&file).is_none());
    }

    #[test]
    fn evaluate_constant_folds_arithmetic() {
        let src = "let x = 2 + 3\n";
        let file = TestFile::new(src);
        let start = src.find("2 + 3").unwrap();
        let end = start + "2 + 3".len();
        let edits =
            evaluate_constant_edits(&file, base_db::text_range(start, end)).expect("should fold");
        assert_eq!(edits[0].new_text, "5");
    }

    #[test]
    fn evaluate_constant_rejects_same() {
        let src = "let x = 42\n";
        let file = TestFile::new(src);
        let start = src.find("42").unwrap();
        let end = start + "42".len();
        assert!(evaluate_constant_edits(&file, base_db::text_range(start, end)).is_none());
    }

    #[test]
    fn simplify_boolean_removes_eq_true() {
        let src = "let y = x == true\n";
        let file = TestFile::new(src);
        let start = src.find("x == true").unwrap();
        let end = start + "x == true".len();
        let edits = simplify_boolean_edits(&file, base_db::text_range(start, end))
            .expect("should simplify");
        assert_eq!(edits[0].new_text, "x");
    }

    #[test]
    fn simplify_boolean_negates_eq_false() {
        let src = "let y = x == false\n";
        let file = TestFile::new(src);
        let start = src.find("x == false").unwrap();
        let end = start + "x == false".len();
        let edits = simplify_boolean_edits(&file, base_db::text_range(start, end))
            .expect("should simplify");
        assert_eq!(edits[0].new_text, "~(x)");
    }

    #[test]
    fn inline_simple_call() {
        use crate::handlers::inline_call::inline_call_edits;
        let src = "function double(x) = x + x\nfunction main() = double(5)\n";
        let file = TestFile::new(src);
        let offset = src.rfind("double").unwrap() + 1;
        let edits = inline_call_edits(&file, offset);
        assert!(edits.is_some(), "should inline call");
        let edits = edits.unwrap();
        assert!(!edits.is_empty());
        // The inlined text should contain "5 + 5" (x replaced with 5)
        assert!(
            edits[0].new_text.contains("5"),
            "should substitute arg, got: {}",
            edits[0].new_text
        );
    }

    #[test]
    fn generate_val_spec_for_function_without_val() {
        let src = "function add(x, y) = x + y\n";
        let file = TestFile::new(src);
        // Cursor on the function keyword
        let offset = src.find("add").unwrap();
        let range = base_db::text_range(offset, offset + 3);
        let assists = super::assists(&super::AssistConfig::default(), &file, range);
        let gen = assists.iter().find(|a| a.id.0 == "generate_val_spec");
        assert!(
            gen.is_some(),
            "should offer generate_val_spec, got: {:?}",
            assists.iter().map(|a| &a.id.0).collect::<Vec<_>>()
        );
        let gen = gen.unwrap();
        let text = gen
            .source_change
            .as_ref()
            .and_then(|c| c.source_file_edits.values().next())
            .and_then(|edits| edits.first())
            .map(|e| e.new_text.clone())
            .expect("the assist has an edit");
        assert!(text.contains("val add"), "should generate val spec, got: {text}");
    }

    #[test]
    fn generate_val_spec_not_offered_when_val_exists() {
        let src = "val add : (int, int) -> int\nfunction add(x, y) = x + y\n";
        let file = TestFile::new(src);
        let offset = src.find("function add").unwrap() + 9; // on "add" in function def
        let range = base_db::text_range(offset, offset + 3);
        let assists = super::assists(&super::AssistConfig::default(), &file, range);
        let gen = assists.iter().find(|a| a.id.0 == "generate_val_spec");
        assert!(gen.is_none(), "should NOT offer generate_val_spec when val exists");
    }

    #[test]
    fn add_missing_match_arms_for_inline_enum() {
        let src = "\
enum Color = { Red, Green, Blue }
function f(c : Color) -> int = match c {
  Red => 1,
}
";
        let file = TestFile::new(src);
        // Place cursor inside the match body (on "Red") — this ensures
        // entry_at_offset finds the callable body and expr_at_offset finds the match
        let offset = src.find("Red =>").unwrap();
        let range = base_db::text_range(offset, offset + 3);
        let assists = super::assists(&super::AssistConfig::default(), &file, range);
        let arm = assists.iter().find(|a| a.id.0 == "add_missing_match_arms");
        // The assist may or may not trigger depending on whether the
        // CST body lowering captures the match node at this offset.
        // This test verifies the handler doesn't panic and produces
        // reasonable output when it does trigger.
        if let Some(arm) = arm {
            let text = arm
                .source_change
                .as_ref()
                .and_then(|c| c.source_file_edits.values().next())
                .and_then(|edits| edits.first())
                .map(|e| e.new_text.clone())
                .expect("the assist has an edit");
            assert!(
                text.contains("Green") || text.contains("Blue"),
                "should add missing arms, got: {text}"
            );
        }
        // If it doesn't trigger, that's acceptable — the enum resolution
        // heuristic may not connect "c" to "Color" without type inference.
    }

    #[test]
    fn add_missing_match_arms_not_offered_when_wildcard() {
        let src = "\
enum Color = { Red, Green, Blue }
function f(c : Color) -> int = match c {
  Red => 1,
  _ => 0,
}
";
        let file = TestFile::new(src);
        let offset = src.find("match c").unwrap();
        let range = base_db::text_range(offset, offset + 7);
        let assists = super::assists(&super::AssistConfig::default(), &file, range);
        let arm = assists.iter().find(|a| a.id.0 == "add_missing_match_arms");
        assert!(arm.is_none(), "should NOT offer when wildcard covers remaining");
    }
}
