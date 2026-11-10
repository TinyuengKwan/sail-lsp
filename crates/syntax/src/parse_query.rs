//! Salsa tracked parse query.
//!
//! Wraps the existing lex→parse→preprocess→lower pipeline as a salsa
//! tracked function so that re-parsing is automatically memoized and
//! only re-runs when the file text actually changes.
//!
//! Salsa 0.25 only emits a runtime capacity setter for a nonzero literal LRU.
//! The large seed capacity enables that API; RootDatabase installs the actual
//! configured policy before any analysis starts.

use std::sync::{Arc, OnceLock};

use base_db::FileText;
use parser::{Span, Token};

use crate::parser_lower::ParsedFile;
use crate::syntax_error::SyntaxError;

/// Result of lexing + parsing a single file.
///
/// ```text
/// pub struct Parse<T> {
///     green: Option<GreenNode>,
///     errors: Option<Arc<[SyntaxError]>>,
///     _ty: PhantomData<fn() -> T>,
/// }
/// ```
///
/// Uses `Arc` for cheap cloning. Equality is by `Arc` pointer identity
/// so salsa can detect when the parse result actually changed.
#[derive(Clone, Debug)]
pub struct ParsedFileData {
    tokens: Arc<TokenCache>,
    /// Rowan GreenNode (lossless, Send+Sync). Create SyntaxNode on demand
    /// via `SyntaxNode::new_root(green.clone())`. Used by hir-def queries.
    pub green: Option<Arc<rowan::GreenNode>>,
    /// Lazy parser/validation diagnostics, shared by clones and derived
    /// only from this immutable parse's tree and parser errors.
    validated_errors: Arc<OnceLock<Option<Arc<[SyntaxError]>>>>,
    // Validation errors depend on the whole updated tree. Only parser errors
    // may be shifted/merged when replacing a token or subtree.
    parser_errors: Arc<[SyntaxError]>,
    fixities: Arc<parser::FixityContext>,
}

#[derive(Debug, Default)]
struct TokenCache {
    source: Arc<str>,
    tokens: OnceLock<Vec<(Token, Span)>>,
}

impl TokenCache {
    fn new(source: &str) -> Arc<Self> {
        Arc::new(Self { source: Arc::from(source), tokens: OnceLock::new() })
    }
}

impl ParsedFileData {
    /// The non-trivia token index is independent of CST position queries.
    /// Clones share its initialization; cancellation leaves it retryable.
    pub fn tokens(&self) -> &Vec<(Token, Span)> {
        self.tokens.tokens.get_or_init(|| {
            let _cancellation = crate::cancellation_scope();
            parser::tokenize(&self.tokens.source)
        })
    }
    /// Parser and validation diagnostics, computed only when requested.
    /// Position queries use the CST without walking the entire file to
    /// validate brackets, documentation and visibility on every edit.
    pub fn errors(&self) -> Option<&[SyntaxError]> {
        self.validated_errors
            .get_or_init(|| {
                let _cancellation = crate::cancellation_scope();
                let mut errors = self.parser_errors.to_vec();
                if let Some(root) = self.syntax_node() {
                    crate::validation::validate(&root, &mut errors);
                }
                errors.sort_by(|a, b| {
                    (a.range().start(), a.range().end(), a.message()).cmp(&(
                        b.range().start(),
                        b.range().end(),
                        b.message(),
                    ))
                });
                (!errors.is_empty()).then(|| errors.into())
            })
            .as_deref()
    }

    /// Parser recovery can affect lowering; later validation diagnostics do
    /// not change the CST's interpretation.
    pub fn has_parse_errors(&self) -> bool {
        !self.parser_errors.is_empty()
    }

    /// Create a SyntaxNode from the stored GreenNode.
    pub fn syntax_node(&self) -> Option<crate::syntax_node::SyntaxNode> {
        let green = self.green.as_ref()?;
        Some(crate::syntax_node::SyntaxNode::new_root(rowan::GreenNode::clone(green)))
    }

    /// Attempt incremental reparsing given a text edit.
    ///
    /// tries incremental first, falls back to full reparse.
    ///
    /// Returns a new `ParsedFileData` with updated green tree + errors.
    /// The token stream always describes the same source as the green tree.
    pub fn reparse(&self, delete: rowan::TextRange, insert: &str) -> ParsedFileData {
        if let Some(result) = self.incremental_reparse(delete, insert) {
            return result;
        }
        self.full_reparse(delete, insert)
    }

    /// A speculative tree for position analysis. No caller of this tree
    /// consumes a token index or whole-file validation results, so retain the
    /// unchanged green subtrees without rebuilding those projections.
    pub fn reparse_syntax(
        &self,
        delete: rowan::TextRange,
        insert: &str,
    ) -> Option<crate::SyntaxNode> {
        let node = self.syntax_node()?;
        if let Some((green, _, _)) = crate::parsing::incremental_reparse_with_fixities(
            &node,
            delete,
            insert,
            self.parser_errors.iter().cloned(),
            &self.fixities,
        ) {
            return Some(crate::SyntaxNode::new_root(green));
        }
        self.full_reparse(delete, insert).syntax_node()
    }

    /// Try incremental reparsing (token or block level).
    fn incremental_reparse(
        &self,
        delete: rowan::TextRange,
        insert: &str,
    ) -> Option<ParsedFileData> {
        self.incremental_reparse_with_text(delete, insert, None)
    }

    fn incremental_reparse_with_text(
        &self,
        delete: rowan::TextRange,
        insert: &str,
        updated_text: Option<&str>,
    ) -> Option<ParsedFileData> {
        let node = self.syntax_node()?;
        let (green, parser_errors, _range) = crate::parsing::incremental_reparse_with_fixities(
            &node,
            delete,
            insert,
            self.parser_errors.iter().cloned(),
            &self.fixities,
        )?;
        let root = crate::syntax_node::SyntaxNode::new_root(green);
        // The tracked query already owns the exact updated source used to
        // compute this edit. Reconstructing it from the lossless tree would
        // traverse every unchanged token again.
        if let Some(text) = updated_text {
            return Some(Self::from_root(text, root, parser_errors, self.fixities.clone()));
        }
        let text = root.text().to_string();
        Some(Self::from_root(&text, root, parser_errors, self.fixities.clone()))
    }

    fn from_root(
        text: &str,
        root: crate::SyntaxNode,
        parser_errors: Vec<SyntaxError>,
        fixities: Arc<parser::FixityContext>,
    ) -> Self {
        Self {
            tokens: TokenCache::new(text),
            green: Some(Arc::new(root.green().into())),
            validated_errors: Arc::default(),
            parser_errors: parser_errors.into(),
            fixities,
        }
    }

    fn from_text(text: &str, fixities: Arc<parser::FixityContext>) -> Self {
        let (root, raw_errors) =
            crate::parsing::parse_text_with_fixities(text, (*fixities).clone());
        let parser_errors = raw_errors
            .iter()
            .map(|e| crate::parsing::parse_error_to_syntax_error(e, text))
            .collect();
        Self::from_root(text, root, parser_errors, fixities)
    }

    /// Full reparse fallback, preserving the file's operator context.
    fn full_reparse(&self, delete: rowan::TextRange, insert: &str) -> ParsedFileData {
        let mut text = self.syntax_node().map(|n| n.text().to_string()).unwrap_or_default();
        let start: usize = delete.start().into();
        let end: usize = delete.end().into();
        if text.is_char_boundary(start) && text.is_char_boundary(end) && start <= end {
            text.replace_range(start..end, insert);
        }
        Self::from_text(&text, self.fixities.clone())
    }
}

impl PartialEq for ParsedFileData {
    fn eq(&self, other: &Self) -> bool {
        self.fixities == other.fixities
            && self.parser_errors == other.parser_errors
            && Arc::ptr_eq(&self.tokens, &other.tokens)
            && match (&self.green, &other.green) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}
impl Eq for ParsedFileData {}

impl std::hash::Hash for ParsedFileData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.tokens), state);
        if let Some(a) = self.green.as_ref() {
            std::ptr::hash(Arc::as_ptr(a), state)
        }
    }
}

/// Salsa tracked function: lex + parse a file.
///
/// Takes a `FileText` salsa input and returns `ParsedFileData` with
/// tokens, green node, and parse errors. Salsa memoizes the result
/// and only re-runs when `file_text.text()` changes.
#[salsa::tracked(returns(ref), lru = 4294967295)]
pub fn parse_file(db: &dyn salsa::Database, input: FileText) -> ParsedFileData {
    let _cancellation = crate::cancellation_scope();
    if let Some(source) = base_db::preprocess::source_text(db, input.file_id(db)) {
        return parse_file(db, source).clone();
    }
    let text = input.text(db);
    let started = std::time::Instant::now();

    let fixities = base_db::WorkspaceFixities::try_get(db)
        .map(|context| context.fixities(db).clone())
        .unwrap_or_default();
    let fixities = Arc::new(fixities);
    let cache = input.parse_cache(db);
    // Do not hold the cache lock while lexing/parsing. Concurrent snapshots
    // may replace the seed in either order; its complete key makes that affect
    // only reuse, never the result computed for this snapshot's inputs.
    let reused = cache.get().filter(|seed| seed.fixities == fixities).and_then(|seed| {
        if seed.text.as_ref() == text.as_ref() {
            // A newly installed singleton may invalidate the query without a
            // text change. There is no edit to reparse in this case.
            return None;
        }
        let green = seed.green.upgrade()?;
        let previous = ParsedFileData {
            tokens: Arc::default(),
            green: Some(green),
            validated_errors: Arc::default(),
            parser_errors: seed
                .parser_errors
                .iter()
                .map(|(message, range)| SyntaxError::new(message.clone(), *range))
                .collect(),
            fixities: fixities.clone(),
        };
        let (delete, insert) = text_difference(&seed.text, text)?;
        previous.incremental_reparse_with_text(delete, insert, Some(text))
    });
    let incremental = reused.is_some();
    let parsed = reused.unwrap_or_else(|| ParsedFileData::from_text(text, fixities.clone()));
    if text.len() >= 1_000_000 {
        tracing::debug!(target: "sail_lsp::latency", "parse_file file={:?} bytes={} incremental={incremental} parse_us={}", input.file_id(db), text.len(), started.elapsed().as_micros());
    }
    if let Some(green) = &parsed.green {
        cache.store(base_db::parse_cache::ParseSeed {
            text: text.clone(),
            green: Arc::downgrade(green),
            parser_errors: parsed
                .parser_errors
                .iter()
                .map(|error| (error.message().to_owned(), error.range()))
                .collect(),
            fixities,
        });
    }
    parsed
}

/// A single covering edit for a batch of LSP changes, with UTF-8 boundaries.
/// Structural edits fall back to a complete parse if no safe token/block
/// replacement or verified batch of token replacements covers the change.
fn text_difference<'a>(before: &str, after: &'a str) -> Option<(rowan::TextRange, &'a str)> {
    let mut start = before.bytes().zip(after.bytes()).take_while(|(a, b)| a == b).count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = before[start..]
        .bytes()
        .rev()
        .zip(after[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    Some((
        rowan::TextRange::new(
            u32::try_from(start).ok()?.into(),
            u32::try_from(before.len() - suffix).ok()?.into(),
        ),
        &after[start..after.len() - suffix],
    ))
}

/// Keep source positions while removing declarations in inactive branches.
/// The raw CST remains available for editing, formatting and syntax errors.
#[salsa::tracked(returns(ref), lru = 4294967295)]
pub fn semantic_parse_file(db: &dyn salsa::Database, input: FileText) -> ParsedFileData {
    let _cancellation = crate::cancellation_scope();
    let parsed = parse_file(db, input);
    let Some(regions) = base_db::preprocess::file_regions(db, input.file_id(db)) else {
        return parsed.clone();
    };
    let Some(root) = parsed.syntax_node() else { return parsed.clone() };
    let mut index = 0;
    let mut text = input.text(db).as_bytes().to_vec();
    let mut changed = false;
    for node in root.children() {
        let event = crate::preprocess::Event::from_node(&node);
        let enabled = regions.get(index).copied().unwrap_or(false);
        if !enabled || event.as_ref().is_some_and(|e| e.consumed()) {
            let range = node.text_range();
            for byte in &mut text[usize::from(range.start())..usize::from(range.end())] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
            changed = true;
        }
        if event.is_some() {
            index += 1;
        }
    }
    if !changed {
        return parsed.clone();
    }
    let text = String::from_utf8(text).expect("whole CST nodes have UTF-8 boundaries");
    let (root, _) = if let Some(fixities) = base_db::WorkspaceFixities::try_get(db) {
        crate::parsing::parse_text_with_fixities(&text, fixities.fixities(db).clone())
    } else {
        crate::parsing::parse_text(&text)
    };
    ParsedFileData {
        tokens: TokenCache::new(&text),
        green: Some(Arc::new(root.green().into())),
        validated_errors: parsed.validated_errors.clone(),
        parser_errors: parsed.parser_errors.clone(),
        fixities: parsed.fixities.clone(),
    }
}

/// Firewall query: extract parse errors from the cached parse result.
///
/// ```text
/// #[salsa::tracked(returns(as_deref))]
/// pub fn parse_errors(db, file_id) -> Option<Box<[SyntaxError]>> {
///     let errors = file_id.parse(db).errors();
///     match &*errors { [] => None, [..] => Some(errors.into()) }
/// }
/// ```
///
/// This is a separate tracked query so that downstream consumers
/// (diagnostics) don't re-run when the parse changed but errors didn't.
/// Returns `Option<&[SyntaxError]>` via `returns(as_deref)`.
#[salsa::tracked(returns(as_deref))]
pub fn parse_errors(db: &dyn salsa::Database, input: FileText) -> Option<Box<[SyntaxError]>> {
    let parsed = parse_file(db, input);
    parsed.errors().map(|errors| errors.iter().cloned().collect())
}

/// Wrapper for `Arc<ParsedFile>` with pointer-based Eq/Hash for salsa.
#[derive(Clone, Debug)]
pub struct ArcParsedFile(pub Arc<ParsedFile>);

impl PartialEq for ArcParsedFile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcParsedFile {}
impl std::hash::Hash for ArcParsedFile {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

/// Salsa tracked function: build the semantic ParsedFile index.
///
/// `ParsedFile` contains callable_heads, type_aliases, call_sites,
/// typed_bindings, symbol_occurrences — the structured semantic view
/// used by `collect_callable_signatures`, `build_signature_index`, etc.
///
/// Build ParsedFile from CST (lossless). This is the production path.
#[salsa::tracked(returns(ref))]
pub fn parsed_file(db: &dyn salsa::Database, input: FileText) -> Option<ArcParsedFile> {
    let _cancellation = crate::cancellation_scope();
    let text = input.text(db);
    if text.is_empty() {
        return None;
    }
    let root = semantic_parse_file(db, input).syntax_node()?;
    Some(ArcParsedFile(Arc::new(crate::cst_lower::parsed_file_from_cst(&root, text.as_ref()))))
}

/// Updated by the host while no query snapshots are active.
pub fn set_parse_cache_capacity(db: &mut dyn salsa::Database, capacity: usize) {
    // Keep usage tracking when eviction is disabled, so a later pressure
    // limit can reclaim already cached values rather than only new reads.
    let capacity = if capacity == 0 { usize::MAX } else { capacity };
    parse_file::set_lru_capacity(db, capacity);
    semantic_parse_file::set_lru_capacity(db, capacity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::FileId;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[test]
    fn token_index_is_lazy_shared_and_bound_to_its_source_snapshot() {
        use salsa::Setter;
        use std::hash::{Hash, Hasher};
        let mut db = TestDb::default();
        let before = "// 茶\nfunction f(x : int) = x + 1\n";
        let after = before.replace("x + 1", "x + 2");
        let input = FileText::new(&db, Arc::from(before), FileId::from_raw(0));
        let parsed = parse_file(&db, input).clone();
        let clone = parsed.clone();
        let hash = |value: &ParsedFileData| {
            let mut state = std::collections::hash_map::DefaultHasher::new();
            value.hash(&mut state);
            state.finish()
        };
        let original_hash = hash(&parsed);
        assert!(parsed.tokens.tokens.get().is_none());
        assert!(parsed.syntax_node().is_some());
        assert!(parsed.tokens.tokens.get().is_none());
        input.set_text(&mut db).to(Arc::from(after.as_str()));
        let edited = parse_file(&db, input);
        assert!(edited.tokens.tokens.get().is_none());
        assert_eq!(parsed.tokens(), &parser::tokenize(before));
        assert!(std::ptr::eq(parsed.tokens(), clone.tokens()));
        assert_eq!(parsed, clone);
        assert_eq!(hash(&parsed), original_hash);
        assert_eq!(edited.tokens(), &parser::tokenize(&after));
    }

    #[test]
    fn parse_simple_function() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("function foo(x : int) -> int = x + 1"),
            FileId::from_raw(0),
        );

        let parsed = parse_file(&db, input);
        assert!(!parsed.tokens().is_empty(), "should produce tokens");
        assert!(parsed.green.is_some(), "should produce a GreenNode");
    }

    #[test]
    fn parse_val_declaration() {
        let db = TestDb::default();
        let input =
            FileText::new(&db, Arc::from("val add : (int, int) -> int\n"), FileId::from_raw(0));

        let parsed = parse_file(&db, input);
        assert!(parsed.green.is_some());
    }

    #[test]
    fn parse_empty_file() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from(""), FileId::from_raw(0));

        let parsed = parse_file(&db, input);
        // Empty text → empty tokens, no AST
        assert!(parsed.tokens().is_empty());
    }

    #[test]
    fn parse_is_memoized() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));

        // Call twice — second call should return the same Arc (memoized)
        let r1 = parse_file(&db, input);
        let r2 = parse_file(&db, input);
        assert!(Arc::ptr_eq(&r1.tokens, &r2.tokens));
    }

    #[test]
    fn parse_invalidates_on_text_change() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));

        let r1 = parse_file(&db, input).clone();
        assert!(r1.green.is_some());

        // Change the file text
        input.set_text(&mut db).to(Arc::from("val x : bool\n"));

        let r2 = parse_file(&db, input);
        assert!(r2.green.is_some());

        // Tokens should be different (not same Arc pointer)
        assert!(!Arc::ptr_eq(&r1.tokens, &r2.tokens));
    }

    #[test]
    fn parsed_file_has_callable_heads() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("val foo : int -> int\nfunction foo(x) = x + 1\n"),
            FileId::from_raw(0),
        );

        let pf = parsed_file(&db, input);
        assert!(pf.is_some(), "should produce ParsedFile");
        let pf = &pf.as_ref().unwrap().0;
        assert!(!pf.callable_heads.is_empty(), "should have callable heads");
    }

    #[test]
    fn normal_text_updates_reuse_trees_and_fixity_changes_do_not() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let fixities = base_db::WorkspaceFixities::new(&db, 0, Default::default());
        let before = "function f(x) = { x =^ 2 + 3 }\nfunction untouched() = 4\n";
        let input = FileText::new(&db, Arc::from(before), FileId::from_raw(0));
        let untouched_before = parse_file(&db, input)
            .syntax_node()
            .unwrap()
            .children()
            .nth(1)
            .unwrap()
            .green()
            .into_owned();
        let after = before.replace("2", "5 * 6");
        input.set_text(&mut db).to(Arc::from(after.as_str()));
        let parsed = parse_file(&db, input).clone();
        let untouched_after =
            parsed.syntax_node().unwrap().children().nth(1).unwrap().green().into_owned();
        assert!(
            std::ptr::eq(&*untouched_before, &*untouched_after),
            "normal editing did not reuse the unaffected subtree"
        );
        let full = ParsedFileData::from_text(&after, Arc::default());
        assert_eq!(parsed.green, full.green);
        assert_eq!(parsed.tokens(), full.tokens());
        assert_eq!(parsed.errors(), full.errors());

        let context = std::collections::HashMap::from([("=^".into(), (19, 20))]);
        fixities.set_fixities(&mut db).to(context.clone());
        let updated = parse_file(&db, input);
        let full = ParsedFileData::from_text(&after, Arc::new(context));
        assert_eq!(updated.green, full.green);
        assert_ne!(updated.green, parsed.green, "new operator precedence must change the tree");
        assert_eq!(updated.tokens(), full.tokens());
        assert_eq!(updated.errors(), full.errors());
    }

    #[test]
    fn unchanged_mapping_text_retains_clause_grammar_after_context_installation() {
        let mut db = TestDb::default();
        let source = "mapping m : int <-> bool = { forwards x if x > 0 => true when x < 5, backwards _ => 0 }";
        let input = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
        let before = parse_file(&db, input).clone();
        base_db::WorkspaceFixities::new(&db, 0, Default::default());
        // Force a revision just as workspace registration does.
        use salsa::Database;
        db.trigger_cancellation();
        let after = parse_file(&db, input);
        assert_eq!(before.green, after.green);
        assert_eq!(before.errors(), after.errors());
    }

    #[test]
    fn reparse_seed_does_not_keep_an_evicted_tree_alive() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("function f() = 1"), FileId::from_raw(0));
        parse_file(&db, input);
        let cache = input.parse_cache(&db).clone();
        let seed = cache.get().unwrap();
        assert!(seed.green.upgrade().is_some());
        drop(db);
        assert!(seed.green.upgrade().is_none(), "seed pinned a tree after database disposal");
        cache.clear();
        assert!(cache.get().is_none());
    }

    #[test]
    fn cached_normal_edits_match_full_parse_after_unicode_and_undo() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let before = "// café\nfunction f(x) = { x + 1 }\n";
        let input = FileText::new(&db, Arc::from(before), FileId::from_raw(0));
        parse_file(&db, input);
        for after in [
            before.replace("café", "茶"),
            before.replace("x + 1", "(x +"),
            before.replace("x + 1", "x + 2 + 3"),
            before.to_owned(),
        ] {
            input.set_text(&mut db).to(Arc::from(after.as_str()));
            let actual = parse_file(&db, input);
            let full = ParsedFileData::from_text(&after, Arc::default());
            assert_eq!(actual.green, full.green, "{after}");
            assert_eq!(actual.tokens(), full.tokens(), "{after}");
            assert_eq!(actual.errors(), full.errors(), "{after}");
        }
    }

    #[test]
    fn incremental_reparse_ident() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("function foo(x : int) -> int = x + 1\n"),
            FileId::from_raw(0),
        );
        let parsed = parse_file(&db, input).clone();
        assert!(parsed.green.is_some());

        // Change "foo" to "bar" — should succeed as token-level reparse
        let delete = rowan::TextRange::new(9.into(), 12.into()); // "foo"
        let reparsed = parsed.reparse(delete, "bar");
        assert!(reparsed.green.is_some());

        // Verify lossless: new tree text contains "bar"
        let node = reparsed.syntax_node().unwrap();
        let text = node.text().to_string();
        assert!(text.contains("bar"), "reparsed tree should contain 'bar': {text}");
        assert!(!text.contains("foo"), "reparsed tree should not contain 'foo': {text}");
        assert_eq!(*reparsed.tokens(), parser::tokenize(&text));

        // A different byte length must also shift following token spans.
        let reparsed = reparsed.reparse(delete, "longer_name");
        let text = reparsed.syntax_node().unwrap().text().to_string();
        assert_eq!(*reparsed.tokens(), parser::tokenize(&text));
    }

    #[test]
    fn validation_is_lazy_and_preserves_parse_identity() {
        use std::hash::{Hash, Hasher};
        let parsed =
            ParsedFileData::from_text("function f() = { if true { 1 } }\n", Arc::default());
        let other = parsed.clone();
        assert!(parsed.validated_errors.get().is_none());
        let hash = |parsed: &ParsedFileData| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            parsed.hash(&mut hasher);
            hasher.finish()
        };
        let before = hash(&parsed);
        assert!(parsed.errors().unwrap().iter().any(|error| error.message().contains("then")));
        assert_eq!(before, hash(&parsed));
        assert_eq!(parsed, other);
        assert!(std::ptr::eq(parsed.errors().unwrap(), other.errors().unwrap()));
    }

    #[test]
    fn cached_privacy_edits_cross_empty_recovery_nodes_safely() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let public = "union U = { Hidden : unit, Public : unit }\n";
        let private = public.replace("Hidden", "private Hidden");
        let file = FileText::new(&db, Arc::from(public), FileId::from_raw(0));
        parse_file(&db, file);
        for text in [&*private, public, &*private, public] {
            file.set_text(&mut db).to(Arc::from(text));
            let cached = parse_file(&db, file);
            let full = ParsedFileData::from_text(text, Arc::default());
            assert_eq!(cached.green, full.green);
            assert_eq!(cached.tokens(), full.tokens());
            assert_eq!(cached.errors(), full.errors());
        }
    }

    #[test]
    fn disjoint_renames_and_numeric_edits_keep_incremental_trees_equal_to_full_parses() {
        let base = "// 茶\nfunction target(x : int) = x + 0\nfunction use() = target(1)\n";
        let mut previous = base.to_owned();
        let mut parsed = ParsedFileData::from_text(&previous, Arc::default());
        for after in [
            base.replace("target", "target_renamed"),
            base.replace(" + 0", " + 1"),
            base.replace("target", "short").replace(" + 0", " + 42"),
            base.to_owned(),
        ] {
            let (delete, insert) = text_difference(&previous, &after).unwrap();
            let incremental = parsed
                .incremental_reparse(delete, insert)
                .expect("safe token edits must be incremental");
            let full = ParsedFileData::from_text(&after, Arc::default());
            assert_eq!(incremental.green, full.green, "{after}");
            assert_eq!(incremental.tokens(), full.tokens(), "{after}");
            assert_eq!(incremental.errors(), full.errors(), "{after}");
            parsed = incremental;
            previous = after;
        }
        // A changed operator or inserted delimiter is a structural edit,
        // even when the surrounding names also changed.
        for after in [
            base.replace("target", "short").replace(" + 0", " * 0"),
            base.replace("target(1)", "short((1))").replace("function target", "function short"),
        ] {
            let (delete, insert) = text_difference(base, &after).unwrap();
            assert!(parsed.incremental_reparse(delete, insert).is_none());
        }
    }

    #[test]
    fn reparsing_preserves_fixities_and_refreshes_validation() {
        let fixities = Arc::new(std::collections::HashMap::from([("=^".into(), (19, 20))]));
        for (before, needle, replacement) in [
            ("function f(x) = { x =^ 2 + 3 }\n", "2", "4 * 5"),
            ("function f(x) = x =^ 2 + 3\n", "x =^ 2", "(x =^ 2)"),
            ("/// first\n\n/// second\nfunction f() = ()\n", "second", "updated comment"),
            ("function target(x) = { if x { 1 } }\n", "target", "longer_name"),
            ("function f(x) = { if x { 1 } }\n", "1", "2 + 3"),
        ] {
            let parsed = ParsedFileData::from_text(before, fixities.clone());
            let start = before.find(needle).unwrap();
            let range = rowan::TextRange::new(
                (start as u32).into(),
                ((start + needle.len()) as u32).into(),
            );
            if needle == "2" {
                assert!(
                    parsed.incremental_reparse(range, replacement).is_some(),
                    "incremental path not exercised: {before}"
                );
            }
            let actual = parsed.reparse(range, replacement);
            let after = before.replacen(needle, replacement, 1);
            let expected = ParsedFileData::from_text(&after, fixities.clone());
            assert_eq!(actual.green, expected.green, "{after}");
            assert_eq!(actual.tokens(), expected.tokens(), "{after}");
            assert_eq!(actual.errors(), expected.errors(), "{after}");
        }
    }

    #[test]
    fn incremental_edits_match_full_parse_and_undo() {
        let fixities = Arc::new(std::collections::HashMap::from([("=^".into(), (19, 20))]));
        for before in [
            "mapping m : int <-> bool = { forwards x if x > 0 => true when x < 5, backwards _ => 0 }",
            "mapping m : (int, int) <-> bool = { (1, 2, 3) <-> true }",
            "mapping m : int <-> bool = {}",
            "function f(x) = { x =^ 2 + 3 }\n",
            "// café\nfunction f() = \"α\"\n",
            "function f(x) = { if x then (1 }\n",
            "function a() = { é }\nfunction b() = { é }\n",
            "function a() = { if true }\nfunction b() = { if false }\n",
            "/// doc\n\n/// another\nval f : int -> int\n",
        ] {
            let parsed = ParsedFileData::from_text(before, fixities.clone());
            for start in before.char_indices().map(|(i, _)| i).chain([before.len()]) {
                let next = before[start..].chars().next().map_or(start, |c| start + c.len_utf8());
                for end in [start, next] {
                    for replacement in ["", " ", "\n", "x", "{", "}", "//", "\"", "é"] {
                        let range =
                            rowan::TextRange::new((start as u32).into(), (end as u32).into());
                        let actual = parsed.reparse(range, replacement);
                        let mut after = before.to_owned();
                        after.replace_range(start..end, replacement);
                        let expected = ParsedFileData::from_text(&after, fixities.clone());
                        assert_eq!(
                            actual.green, expected.green,
                            "edit {start}..{end} -> {replacement:?}: {before:?}"
                        );
                        assert_eq!(actual.tokens(), expected.tokens(), "{after:?}");
                        assert_eq!(actual.errors(), expected.errors(), "{after:?}");
                        let undo_range = rowan::TextRange::new(
                            (start as u32).into(),
                            ((start + replacement.len()) as u32).into(),
                        );
                        let undone = actual.reparse(undo_range, &before[start..end]);
                        assert_eq!(undone.green, parsed.green, "undo {after:?}");
                        assert_eq!(undone.tokens(), parsed.tokens(), "undo {after:?}");
                        assert_eq!(undone.errors(), parsed.errors(), "undo {after:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn incremental_reparse_falls_back_to_full() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("function foo(x : int) -> int = x + 1\n"),
            FileId::from_raw(0),
        );
        let parsed = parse_file(&db, input).clone();

        // Insert a new line — likely needs full reparse
        let len: rowan::TextSize = parsed.syntax_node().unwrap().text_range().len();
        let end = len;
        let delete = rowan::TextRange::new(end, end);
        let reparsed = parsed.reparse(delete, "\nval y : int\n");
        assert!(reparsed.green.is_some());

        let text = reparsed.syntax_node().unwrap().text().to_string();
        assert!(text.contains("val y : int"), "reparsed should contain new text");
    }
}
