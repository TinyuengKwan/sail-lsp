//! Salsa tracked queries for the IDE database layer.
//!
//! Builds on `syntax::parse_query` to derive higher-level indices
//! needed by IDE features (signature index, ref counts, etc.).
//!
//! Created in stage a.

use std::sync::Arc;

use base_db::FileText;
use syntax::parse_query::parsed_file;

/// The file's `ParsedFile`, with the local bindings the CST lowering does not
/// record.
///
/// `syntax::parse_query::parsed_file` sees only what the CST carries;
/// let/var/parameter bindings live in the `Body` arenas. Hover and
/// find-references resolve a local name through `ParsedFile::decls`, so
/// without this step neither can say anything about a local.
///
/// The enrichment existed but had no caller outside a test helper, so the
/// server never got it — only tests did, which made hover on a local look
/// like it worked.
#[salsa::tracked(returns(ref))]
pub fn enriched_parsed_file(
    db: &dyn salsa::Database,
    input: FileText,
) -> Option<Arc<syntax::parser_lower::ParsedFile>> {
    let base = parsed_file(db, input).as_ref()?.0.as_ref().clone();
    let mut parsed = base;
    if let Some(bodies) = hir::def_query::callable_bodies(db, input).as_ref() {
        crate::helpers::enrich_parsed_with_local_bindings(&mut parsed, &bodies.0);
    }
    Some(Arc::new(parsed))
}

/// Per-file line index — maps byte offsets to line/column positions.
///
/// Salsa ensures this is only recomputed when file text changes.
#[salsa::tracked(returns(ref))]
pub fn line_index(db: &dyn salsa::Database, input: FileText) -> Arc<crate::line_index::LineIndex> {
    let text = input.text(db);
    Arc::new(crate::line_index::LineIndex::new(text.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::FileId;
    use hir::semantic_queries::signature_index;

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

/// The symbols one file declares, calls and implements.
///
/// rust-analyzer's symbol index is a salsa query — `file_symbols` per file,
/// merged per source root. Ours was a `HashMap` in `GlobalState`, updated by
/// hand from a dirty-file list and deep-cloned on every batch: 8.9 ms on
/// sail-riscv, on the main loop's thread, after every settled edit.
#[salsa::tracked(returns(ref))]
pub fn file_symbols(
    db: &dyn salsa::Database,
    input: FileText,
) -> Arc<crate::symbol_index::FileSymbols> {
    let file_id = input.file_id(db);
    Arc::new(crate::symbol_index::FileSymbols::of(db, file_id, input))
}

/// The workspace's symbol index, merged from every file's symbols.
///
/// Equality is a fingerprint over the per-file results' identities, not a
/// comparison of the merged map: an unchanged file's `file_symbols` is
/// verified rather than re-executed, so its `Arc` is the same one, and
/// hashing 158 pointers is what tells the merge it can be reused.
#[derive(Clone, Debug)]
pub struct ArcSymbolIndex(pub Arc<crate::symbol_index::SymbolIndex>, u64);

impl PartialEq for ArcSymbolIndex {
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1
    }
}
impl Eq for ArcSymbolIndex {}
impl std::hash::Hash for ArcSymbolIndex {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.1.hash(state);
    }
}

#[salsa::tracked(returns(ref))]
pub fn workspace_symbol_index(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
) -> ArcSymbolIndex {
    use std::hash::{Hash, Hasher};

    let mut index = crate::symbol_index::SymbolIndex::new();
    let mut hasher = rustc_hash::FxHasher::default();
    for &ft in ws.file_texts(db) {
        let symbols = file_symbols(db, ft);
        std::ptr::hash(Arc::as_ptr(symbols), &mut hasher);
        ft.file_id(db).hash(&mut hasher);
        index.insert_file(ft.file_id(db), symbols);
    }
    ArcSymbolIndex(Arc::new(index), hasher.finish())
}

#[salsa::tracked(returns(ref))]
fn visible_file_symbols(
    db: &dyn salsa::Database,
    file: FileText,
    from: base_db::FileId,
) -> Arc<crate::symbol_index::FileSymbols> {
    let mut symbols = (**file_symbols(db, file)).clone();
    symbols
        .entries
        .retain(|entry| hir::visibility::symbol_visible_from(db, from, file, &entry.name));
    Arc::new(symbols)
}

/// Position-based navigation sees the same module scope as inference.
/// Workspace-wide symbol search intentionally retains its global index.
#[salsa::tracked(returns(ref))]
pub fn visible_symbol_index(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    from: base_db::FileId,
) -> ArcSymbolIndex {
    use std::hash::{Hash, Hasher};
    let mut index = crate::symbol_index::SymbolIndex::new();
    let mut hasher = rustc_hash::FxHasher::default();
    for &file in ws.file_texts(db) {
        if !base_db::module_graph::can_reference_file(db, from, file.file_id(db)) {
            continue;
        }
        let symbols = visible_file_symbols(db, file, from);
        std::ptr::hash(Arc::as_ptr(symbols), &mut hasher);
        file.file_id(db).hash(&mut hasher);
        index.insert_file(file.file_id(db), symbols);
    }
    ArcSymbolIndex(Arc::new(index), hasher.finish())
}
