//! Salsa tracked parse query.
//!
//! Wraps the existing lex→parse→preprocess→lower pipeline as a salsa
//! tracked function so that re-parsing is automatically memoized and
//! only re-runs when the file text actually changes.
//!
//! Created in stage .

use std::sync::Arc;

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
    pub tokens: Arc<Vec<(Token, Span)>>,
    /// Rowan GreenNode (lossless, Send+Sync). Create SyntaxNode on demand
    /// via `SyntaxNode::new_root(green.clone())`. Used by hir-def queries.
    pub green: Option<Arc<rowan::GreenNode>>,
    /// Parse errors collected during CST construction.
    ///
    /// `None` when there are no errors (common case — avoids Arc alloc).
    pub errors: Option<Arc<[SyntaxError]>>,
}

impl ParsedFileData {
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
    /// Tokens are NOT updated (caller should re-lex if needed).
    pub fn reparse(&self, delete: rowan::TextRange, insert: &str) -> ParsedFileData {
        if let Some(result) = self.incremental_reparse(delete, insert) {
            return result;
        }
        self.full_reparse(delete, insert)
    }

    /// Try incremental reparsing (token or block level).
    fn incremental_reparse(
        &self,
        delete: rowan::TextRange,
        insert: &str,
    ) -> Option<ParsedFileData> {
        let node = self.syntax_node()?;
        let old_errors = self.errors.as_deref().map(|e| e.to_vec()).unwrap_or_default();

        let (green, errors, _range) =
            crate::parsing::incremental_reparse(&node, delete, insert, old_errors)?;

        Some(ParsedFileData {
            tokens: self.tokens.clone(), // tokens not re-lexed in incremental path
            green: Some(Arc::new(green)),
            errors: if errors.is_empty() { None } else { Some(errors.into()) },
        })
    }

    /// Full reparse fallback.
    fn full_reparse(&self, delete: rowan::TextRange, insert: &str) -> ParsedFileData {
        let node = self.syntax_node();
        let old_text = node.as_ref().map(|n| n.text().to_string()).unwrap_or_default();

        let mut text = old_text;
        let start: usize = delete.start().into();
        let end: usize = delete.end().into();
        if start <= text.len() && end <= text.len() {
            text.replace_range(start..end, insert);
        }

        let tokens = parser::tokenize(&text);
        let (root, raw_errors) = crate::parsing::parse_text(&text);
        let green = Some(Arc::new(root.green().into()));
        let errors = if raw_errors.is_empty() {
            None
        } else {
            let syntax_errors: Vec<SyntaxError> = raw_errors
                .iter()
                .map(|e| crate::parsing::parse_error_to_syntax_error(e, &text))
                .collect();
            Some(syntax_errors.into())
        };

        ParsedFileData { tokens: Arc::new(tokens), green, errors }
    }
}

impl PartialEq for ParsedFileData {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.tokens, &other.tokens)
            && match (&self.green, &other.green) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && match (&self.errors, &other.errors) {
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
        if let Some(a) = self.errors.as_ref() {
            std::ptr::hash(Arc::as_ptr(a), state)
        }
    }
}

/// Salsa tracked function: lex + parse a file.
///
/// Takes a `FileText` salsa input and returns `ParsedFileData` with
/// tokens, green node, and parse errors. Salsa memoizes the result
/// and only re-runs when `file_text.text()` changes.
#[salsa::tracked(returns(ref))]
pub fn parse_file(db: &dyn salsa::Database, input: FileText) -> ParsedFileData {
    let text = input.text(db);

    // Step 1: lex (hand-written tokenizer)
    let tokens: Vec<(Token, Span)> = parser::tokenize(text.as_ref());

    // Step 2: CST parse (lossless rowan GreenNode)
    // Use workspace fixities for dynamic operator precedence if available.
    // Establishes salsa dependency: WorkspaceFixities changes → re-parse.
    let (root, raw_errors) = if let Some(ws_fix) = base_db::WorkspaceFixities::try_get(db) {
        let fixities = ws_fix.fixities(db);
        if fixities.is_empty() {
            crate::parsing::parse_text(text.as_ref())
        } else {
            crate::parsing::parse_text_with_fixities(text.as_ref(), fixities.clone())
        }
    } else {
        crate::parsing::parse_text(text.as_ref())
    };
    let green = Some(Arc::new(root.green().into()));

    // Step 3: Convert parse errors to SyntaxError.
    //   errors: if errors.is_empty() { None } else { Some(errors.into()) }
    let errors = if raw_errors.is_empty() {
        None
    } else {
        let syntax_errors: Vec<SyntaxError> = raw_errors
            .iter()
            .map(|e| crate::parsing::parse_error_to_syntax_error(e, text.as_ref()))
            .collect();
        Some(syntax_errors.into())
    };

    ParsedFileData { tokens: Arc::new(tokens), green, errors }
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
    parsed.errors.as_ref().map(|errors| errors.iter().cloned().collect())
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
    let text = input.text(db);
    if text.is_empty() {
        return None;
    }
    let (root, _) = crate::parsing::parse_text(text.as_ref());
    Some(ArcParsedFile(Arc::new(crate::cst_lower::parsed_file_from_cst(&root, text.as_ref()))))
}

/// CST-based ParsedFile extraction . Available for testing and
/// gradual migration. Will replace `parsed_file` once parity
/// is validated.
#[salsa::tracked(returns(ref))]
pub fn parsed_file_cst(db: &dyn salsa::Database, input: FileText) -> Option<ArcParsedFile> {
    let text = input.text(db);
    let (cst_root, _errors) = crate::parsing::parse_text(text.as_ref());
    let cst_parsed = crate::cst_lower::parsed_file_from_cst(&cst_root, text.as_ref());
    if cst_parsed.decls.is_empty() && !text.is_empty() {
        return None;
    }
    Some(ArcParsedFile(Arc::new(cst_parsed)))
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
    fn parse_simple_function() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("function foo(x : int) -> int = x + 1"),
            FileId::from_raw(0),
        );

        let parsed = parse_file(&db, input);
        assert!(!parsed.tokens.is_empty(), "should produce tokens");
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
        assert!(parsed.tokens.is_empty());
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
