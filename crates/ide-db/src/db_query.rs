//! Salsa tracked queries for the IDE database layer.
//!
//! Builds on `syntax::parse_query` to derive higher-level indices
//! needed by IDE features (signature index, ref counts, etc.).
//!
//! Created in stage a.

use std::collections::HashMap;
use std::sync::Arc;

use base_db::FileText;
use syntax::parse_query::parsed_file;

use crate::symbol_index::build_signature_index_from_parsed;
use crate::CallableSignature;

/// Salsa tracked function: build the per-file signature index.
///
/// Maps callable name → CallableSignature. Used by completion,
/// hover, signature help, and many other IDE features.
///
/// RA-1: This query depends on `parsed_file` (which captures
/// callable heads from CST). Body changes don't affect signatures
/// because `parsed_file` only changes when the CST structure
/// changes (not when expression bodies change — that's handled by
/// `infer` separately).
#[salsa::tracked(returns(ref))]
pub fn signature_index(
    db: &dyn salsa::Database,
    input: FileText,
) -> Arc<HashMap<String, CallableSignature>> {
    let pf = parsed_file(db, input);
    let text = input.text(db);
    match pf.as_ref() {
        Some(pf) => Arc::new(build_signature_index_from_parsed(&pf.0, text.as_ref())),
        None => Arc::new(HashMap::new()),
    }
}

/// Per-file reference counts via salsa.
/// Counts symbol reference occurrences from ParsedFile.
#[salsa::tracked(returns(ref))]
pub fn ref_counts(db: &dyn salsa::Database, input: FileText) -> Arc<HashMap<String, usize>> {
    let pf = parsed_file(db, input);
    let mut counts = HashMap::new();
    if let Some(pf) = pf.as_ref() {
        for occ in &pf.0.symbol_occurrences {
            if occ.kind == syntax::parser_lower::SymbolOccurrenceKind::Value && occ.role.is_none()
            // references only, not declarations
            {
                *counts.entry(occ.name.clone()).or_insert(0) += 1;
            }
        }
    }
    Arc::new(counts)
}

/// Per-file implementation counts via salsa.
/// Counts function clause/implementation occurrences.
#[salsa::tracked(returns(ref))]
pub fn impl_counts(db: &dyn salsa::Database, input: FileText) -> Arc<HashMap<String, usize>> {
    let pf = parsed_file(db, input);
    let mut counts = HashMap::new();
    if let Some(pf) = pf.as_ref() {
        for occ in &pf.0.symbol_occurrences {
            if occ.role == Some(syntax::parser_lower::DeclRole::Definition) {
                *counts.entry(occ.name.clone()).or_insert(0) += 1;
            }
        }
    }
    Arc::new(counts)
}

/// Per-file line index — maps byte offsets to line/column positions.
///
/// Salsa ensures this is only recomputed when file text changes.
#[salsa::tracked(returns(ref))]
pub fn line_index(
    db: &dyn salsa::Database,
    input: FileText,
) -> Arc<crate::line_index::LineIndex> {
    let text = input.text(db);
    Arc::new(crate::line_index::LineIndex::new(text.as_ref()))
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
    fn signature_index_contains_function() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("val foo : int -> int\nfunction foo(x) = x + 1\n"),
            FileId::from_raw(0),
        );

        let idx = signature_index(&db, input);
        assert!(idx.contains_key("foo"), "should contain 'foo' signature");
    }

    #[test]
    fn signature_index_memoized() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val bar : bool\n"), FileId::from_raw(0));

        let r1 = signature_index(&db, input);
        let r2 = signature_index(&db, input);
        assert!(Arc::ptr_eq(r1, r2), "should be memoized");
    }

    #[test]
    fn line_index_works() {
        let db = TestDb::default();
        let input =
            FileText::new(&db, Arc::from("val x : int\nfunction f() = 42\n"), FileId::from_raw(0));

        let idx = line_index(&db, input);
        assert_eq!(idx.num_lines(), 3); // 2 lines + trailing
    }

    #[test]
    fn line_index_memoized() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));

        let r1 = line_index(&db, input);
        let r2 = line_index(&db, input);
        assert!(Arc::ptr_eq(r1, r2), "should be memoized");
    }
}
