//! Full-pipeline test utilities for IDE feature crates.
//!
//! `TestFile` runs the complete lex → parse → lower → ItemTree → Body
//! → CallableBodies → CallGraph → ParsedFile pipeline and implements
//! [`FileDb`] so IDE feature tests don't need to depend on
//! `sail_server::state::File`.

use crate::{build_signature_index, CallableSignature, FileDb};
use hir_def::bodies::CallableBodies;
use hir_def::callgraph::{CallGraph, WorkspaceFile};
use hir_def::item_tree::ItemTree;
use parser::{Span, Token};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use syntax::parser_lower::ParsedFile;
/// A self-contained test file that implements [`FileDb`] via the full
/// Sail parse pipeline. Use this in any `#[cfg(test)]` block inside
/// the ide-* crates instead of depending on `sail_server::state::File`.
///
/// ```ignore
/// use ide_db::test_utils::TestFile;
/// let file = TestFile::new("function f(x) = x + 1\n");
/// ```
pub struct TestFile {
    text: String,
    text_hash: u64,
    tokens: Vec<(Token, Span)>,
    parsed: Option<ParsedFile>,
    item_tree: Option<Arc<ItemTree>>,
    bodies: Option<Arc<CallableBodies>>,
    callgraph: Option<Arc<CallGraph>>,
    signature_index: Arc<HashMap<String, CallableSignature>>,
    ref_counts: Arc<HashMap<String, usize>>,
    impl_counts: Arc<HashMap<String, usize>>,
    line_starts: Vec<usize>,
}

impl Clone for TestFile {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            text_hash: self.text_hash,
            tokens: self.tokens.clone(),
            parsed: self.parsed.clone(),
            item_tree: self.item_tree.clone(),
            bodies: self.bodies.clone(),
            callgraph: self.callgraph.clone(),
            signature_index: self.signature_index.clone(),
            ref_counts: self.ref_counts.clone(),
            impl_counts: self.impl_counts.clone(),
            line_starts: self.line_starts.clone(),
        }
    }
}

impl TestFile {
    /// Build a test file from source text, running the full pipeline.
    pub fn new(source: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let text_hash = hasher.finish();

        // hand-written lexer (no chumsky)
        let tokens = parser::tokenize(source);

        let (cst_root, _) = syntax::parse_text(source);
        let mut parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, source);
        let item_tree = Some(Arc::new(ItemTree::build_from_cst(&cst_root)));
        let bodies_arc = Arc::new(CallableBodies::from_cst(&cst_root));
        // Enrich ParsedFile with local bindings from Body arenas
        crate::helpers::enrich_parsed_with_local_bindings(&mut parsed_file, &bodies_arc);
        let parsed = Some(parsed_file);
        let bodies = Some(bodies_arc);

        let callgraph = bodies.as_ref().map(|b| Arc::new(CallGraph::from_callable_bodies(b)));

        // Build signature index through FileDb-compatible path
        let mut line_starts = vec![0usize];
        for (i, ch) in source.char_indices() {
            if ch == '\n' {
                line_starts.push(i + 1);
            }
        }

        // Temporary self to build signature_index
        let mut file = Self {
            text: source.to_string(),
            text_hash,
            tokens,
            parsed,
            item_tree,
            bodies,
            callgraph,
            signature_index: Arc::new(HashMap::new()),
            ref_counts: Arc::new(HashMap::new()),
            impl_counts: Arc::new(HashMap::new()),
            line_starts,
        };

        // Build signature index
        let sig_index = build_signature_index(&file);
        file.signature_index = Arc::new(sig_index);

        // ref_counts and impl_counts are left empty — they're
        // populated by sail_server's full symbol-occurrence scanner
        // which isn't available in the test-utils layer. Feature
        // tests that need them should use sail_server::state::File.

        file
    }

    fn line_for_offset(&self, offset: usize) -> u32 {
        let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        line as u32
    }
}

impl WorkspaceFile for TestFile {
    fn content_hash(&self) -> u64 {
        self.text_hash
    }
    fn callgraph(&self) -> Option<&CallGraph> {
        self.callgraph.as_deref()
    }
}

impl hir_def::callgraph::SourceFileInfo for TestFile {
    fn text(&self) -> &str {
        &self.text
    }
    fn item_tree(&self) -> Option<&hir_def::ItemTree> {
        self.item_tree.as_deref()
    }
}

impl FileDb for TestFile {
    // text() and item_tree() inherited from SourceFileInfo.
    fn position_at(&self, offset: usize) -> crate::LineCol {
        let line = self.line_for_offset(offset);
        let col = (offset - self.line_starts[line as usize]) as u32;
        crate::LineCol { line, col }
    }
    fn offset_at(&self, position: &crate::LineCol) -> usize {
        let line = (position.line as usize).min(self.line_starts.len() - 1);
        (self.line_starts[line] + position.col as usize).min(self.text.len())
    }
    fn tokens(&self) -> Option<&[(Token, Span)]> {
        Some(&self.tokens)
    }
    fn token_at(&self, position: crate::LineCol) -> Option<&(Token, Span)> {
        let offset = self.offset_at(&position);
        self.tokens.iter().rev().find(|(_, span)| span.start <= offset && offset < span.end)
    }
    fn parsed(&self) -> Option<&ParsedFile> {
        self.parsed.as_ref()
    }
    fn signature_index(&self) -> Option<&HashMap<String, CallableSignature>> {
        Some(self.signature_index.as_ref())
    }
    fn ref_counts(&self) -> &HashMap<String, usize> {
        self.ref_counts.as_ref()
    }
    fn impl_counts(&self) -> &HashMap<String, usize> {
        self.impl_counts.as_ref()
    }
    // item_tree() inherited from SourceFileInfo.
    fn bodies(&self) -> Option<&hir_def::bodies::CallableBodies> {
        self.bodies.as_deref()
    }
}
