//! Salsa-backed root database for sail-lsp.
//!
//! `RootDatabase` is the concrete salsa Database that owns file text
//! inputs and derives all per-file analysis through the salsa query
//! chain. `SalsaFile<'a>` is a thin adapter that implements `FileDb`
//! by forwarding to salsa queries — IDE features get the same trait
//! interface whether the backend is the legacy `state::File` or
//! salsa-backed.
//!
//! Created in stage b.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::Arc;

use crate::line_index::{LineCol, LineIndex};
use base_db::{FileText, Files};
use hir_def::callgraph::WorkspaceFile;
use parser::{Span, Token};
use syntax::parser_lower::ParsedFile;

use crate::db_query::signature_index;
use crate::{CallableSignature, FileDb};

/// Monotonically incrementing counter for change tracking.
#[derive(Debug, Clone)]
struct Nonce(u64);

impl Nonce {
    fn new() -> Self {
        Nonce(0)
    }
    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

impl Default for Nonce {
    fn default() -> Self {
        Self::new()
    }
}

/// The concrete salsa database. Owns storage + file text inputs.
/// RA fields: `storage, files: Arc<Files>, crates_map: Arc<CratesMap>, nonce: Nonce`
/// Sail: `storage, nonce` — files remain in external `base_db::Files` during transition.
#[salsa::db]
pub struct RootDatabase {
    /// ManuallyDrop to avoid duplicate vtable instantiation.
    storage: ManuallyDrop<salsa::Storage<Self>>,
    /// Change tracking counter.
    nonce: Nonce,
    /// File text inputs + content-hash dedup.
    files: Files,
}

impl Drop for RootDatabase {
    fn drop(&mut self) {
        // SAFETY: storage is only dropped here, once.
        unsafe { ManuallyDrop::drop(&mut self.storage) }
    }
}

impl Default for RootDatabase {
    fn default() -> Self {
        Self {
            storage: ManuallyDrop::new(salsa::Storage::default()),
            nonce: Nonce::default(),
            files: Files::default(),
        }
    }
}

impl Clone for RootDatabase {
    fn clone(&self) -> Self {
        Self {
            storage: ManuallyDrop::new((*self.storage).clone()),
            nonce: self.nonce.clone(),
            files: self.files.clone(),
        }
    }
}

impl RootDatabase {
    /// Create a new database.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current nonce value (for GC revision tracking).
    pub fn nonce(&self) -> u64 {
        self.nonce.0
    }

    /// Bump the nonce to indicate a change has occurred.
    pub fn bump_nonce(&mut self) -> u64 {
        self.nonce.next()
    }

    /// Access the Files collection (read-only).
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// Access the Files collection (mutable).
    pub fn files_mut(&mut self) -> &mut Files {
        &mut self.files
    }

    /// Apply all pending file changes to salsa.
    ///
    /// This method solves the borrow-conflict where `Files::apply_pending`
    /// needs both `&mut Files` (for draining pending) and `&mut dyn Database`
    /// (for salsa setters). We drain pending into a local vec first, then
    /// do the salsa operations on `self`.
    ///
    /// Returns true if any changes were applied.
    pub fn apply_pending_file_changes(&mut self) -> bool {
        if !self.files.has_pending() {
            return false;
        }
        // Drain pending changes out of Files into a local vec.
        let changes = self.files.take_pending();
        // Now apply each change via salsa (self is the db).
        use salsa::Setter;
        for (file_id, text, durability) in changes {
            if let Some(existing) = self.files.file_text(file_id) {
                existing.set_text(self).with_durability(durability).to(text);
            } else {
                let ft = FileText::builder(text, file_id).durability(durability).new(self);
                self.files.register(file_id, ft);
            }
        }
        true
    }
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

//   SourceDatabase → DefDatabase → HirDatabase → RootDatabase
//
// All methods have default impls in their traits that delegate to
// salsa tracked functions, so these impls are empty.
//
// Note: HirDatabase impl lives in crates/hir (which depends on both
// hir-ty and ide-db), avoiding the hir-ty ↔ ide-db circular dep.

impl base_db::SourceDatabase for RootDatabase {
    fn file_text(&self, file_id: base_db::FileId) -> base_db::FileText {
        self.files
            .file_text(file_id)
            .unwrap_or_else(|| panic!("file_text: unknown FileId {:?}", file_id))
    }

    fn all_file_ids(&self) -> Vec<base_db::FileId> {
        self.files.all_file_ids()
    }

    fn set_file_text(&mut self, file_id: base_db::FileId, text: &str) {
        // Split borrow: query files map first, then mutate self as db.
        let existing = self.files.file_text(file_id);
        let arc_text: Arc<str> = Arc::from(text);
        if let Some(ft) = existing {
            use salsa::Setter;
            ft.set_text(self).to(arc_text);
        } else {
            let ft = FileText::new(self, arc_text, file_id);
            self.files.register(file_id, ft);
        }
    }

    fn set_file_text_with_durability(
        &mut self,
        file_id: base_db::FileId,
        text: &str,
        durability: base_db::Durability,
    ) {
        // Split borrow: query files map first, then mutate self as db.
        let existing = self.files.file_text(file_id);
        let arc_text: Arc<str> = Arc::from(text);
        if let Some(ft) = existing {
            use salsa::Setter;
            ft.set_text(self).with_durability(durability).to(arc_text);
        } else {
            let ft = FileText::builder(arc_text, file_id).durability(durability).new(self);
            self.files.register(file_id, ft);
        }
    }

    fn source_root(&self, _id: base_db::SourceRootId) -> base_db::SourceRootInput {
        unimplemented!("SourceDatabase::source_root — not yet wired")
    }

    fn file_source_root(&self, _id: base_db::FileId) -> base_db::FileSourceRootInput {
        unimplemented!("SourceDatabase::file_source_root — not yet wired")
    }

    fn set_file_source_root_with_durability(
        &mut self,
        _id: base_db::FileId,
        _source_root_id: base_db::SourceRootId,
        _durability: base_db::Durability,
    ) {
        unimplemented!("SourceDatabase::set_file_source_root_with_durability — not yet wired")
    }

    fn set_source_root_with_durability(
        &mut self,
        _source_root_id: base_db::SourceRootId,
        _source_root: Arc<base_db::SourceRoot>,
        _durability: base_db::Durability,
    ) {
        unimplemented!("SourceDatabase::set_source_root_with_durability — not yet wired")
    }
}

impl hir_expand::ExpandDatabase for RootDatabase {
    fn include_paths(&self, input: FileText) -> &[String] {
        hir_expand::include_paths(self, input)
    }
}

/// tracked functions. DefDatabase: ExpandDatabase, so include_paths
/// is inherited from the ExpandDatabase impl above.
impl hir_def::DefDatabase for RootDatabase {
    fn file_item_tree(
        &self,
        input: FileText,
    ) -> Option<&std::sync::Arc<hir_def::item_tree::ItemTree>> {
        hir_def::def_query::file_item_tree(self, input).as_ref()
    }
    fn callable_bodies(&self, input: FileText) -> Option<&hir_def::bodies::CallableBodies> {
        hir_def::def_query::callable_bodies(self, input).as_ref().map(|b| b.0.as_ref())
    }
    fn def_map(&self, input: FileText) -> Option<&hir_def::nameres::DefMap> {
        hir_def::def_query::crate_def_map(self, input).as_ref().map(|d| d.0.as_ref())
    }
    fn callgraph(&self, input: FileText) -> Option<&hir_def::callgraph::CallGraph> {
        hir_def::def_query::callgraph(self, input).as_ref().map(|cg| cg.0.as_ref())
    }
    fn file_def_with_body_ids<'db>(
        &'db self,
        input: FileText,
    ) -> &'db [hir_def::def_query::DefWithBodyId<'db>] {
        hir_def::def_query::file_def_with_body_ids(self, input)
    }
    fn body_with_source_map<'db>(
        &'db self,
        id: hir_def::def_query::DefWithBodyId<'db>,
    ) -> &'db hir_def::def_query::ArcBodyWithSourceMap {
        hir_def::def_query::body_with_source_map(self, id)
    }
}

impl hir_ty::HirDatabase for RootDatabase {
    fn top_level_env(&self, input: FileText) -> &hir_ty::query::ArcTopLevelEnv {
        hir_ty::query::top_level_env(self, input)
    }
    fn infer_body(&self, input: FileText) -> &hir_ty::query::ArcInferenceResult {
        hir_ty::query::infer_body(self, input)
    }
    fn infer<'db>(
        &'db self,
        id: hir_def::def_query::DefWithBodyId<'db>,
    ) -> &'db hir_ty::query::ArcInferenceResult {
        hir_ty::query::infer(self, id)
    }
    fn infer_for_body<'db>(
        &'db self,
        id: hir_def::def_query::DefWithBodyId<'db>,
    ) -> &'db hir_ty::query::ArcInferenceResult {
        hir_ty::query::infer_for_body(self, id)
    }
    fn transitive_effects(&self, input: FileText) -> &hir_ty::query::ArcTransitiveEffects {
        hir_ty::query::transitive_effects(self, input)
    }
}

/// Per-file adapter that implements `FileDb` via salsa queries.
///
/// Constructed from `(&RootDatabase, FileText)`. All methods delegate
/// to the salsa query chain, so results are automatically memoized
/// and invalidated when file text changes.
pub struct SalsaFile<'a> {
    db: &'a RootDatabase,
    input: FileText,
    // Lazily-computed line index for position_at / offset_at.
    line_index: std::cell::OnceCell<LineIndex>,
}

impl<'a> SalsaFile<'a> {
    pub fn new(db: &'a RootDatabase, input: FileText) -> Self {
        Self { db, input, line_index: std::cell::OnceCell::new() }
    }

    fn line_index(&self) -> &LineIndex {
        self.line_index.get_or_init(|| LineIndex::new(self.input.text(self.db).as_ref()))
    }
}

impl<'a> WorkspaceFile for SalsaFile<'a> {
    fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.input.text(self.db).hash(&mut hasher);
        hasher.finish()
    }

    fn callgraph(&self) -> Option<&hir_def::callgraph::CallGraph> {
        hir_def::def_query::callgraph(self.db, self.input).as_ref().map(|acg| acg.0.as_ref())
    }
}

impl<'a> hir_def::callgraph::SourceFileInfo for SalsaFile<'a> {
    fn text(&self) -> &str {
        self.input.text(self.db).as_ref()
    }

    fn item_tree(&self) -> Option<&hir_def::ItemTree> {
        let it = hir_def::def_query::file_item_tree(self.db, self.input);
        it.as_deref()
    }
}

impl<'a> FileDb for SalsaFile<'a> {
    // text() and item_tree() inherited from SourceFileInfo impl above.

    fn position_at(&self, offset: usize) -> LineCol {
        self.line_index().line_col(offset)
    }

    fn offset_at(&self, position: &LineCol) -> usize {
        self.line_index().offset(*position)
    }

    fn tokens(&self) -> Option<&[(Token, Span)]> {
        let parsed = syntax::parse_query::parse_file(self.db, self.input);
        if parsed.tokens.is_empty() {
            None
        } else {
            Some(parsed.tokens.as_slice())
        }
    }

    fn token_at(&self, position: LineCol) -> Option<&(Token, Span)> {
        let offset = self.offset_at(&position);
        let tokens = self.tokens()?;
        tokens.iter().rev().find(|(_, span)| span.start <= offset && offset < span.end)
    }

    fn parsed(&self) -> Option<&ParsedFile> {
        let pf = syntax::parse_query::parsed_file(self.db, self.input);
        pf.as_ref().map(|apf| apf.0.as_ref())
    }

    fn signature_index(&self) -> Option<&HashMap<String, CallableSignature>> {
        Some(signature_index(self.db, self.input).as_ref())
    }

    fn ref_counts(&self) -> &HashMap<String, usize> {
        // Delegate to salsa query (replaces legacy File cache).
        crate::db_query::ref_counts(self.db, self.input).as_ref()
    }

    fn impl_counts(&self) -> &HashMap<String, usize> {
        crate::db_query::impl_counts(self.db, self.input).as_ref()
    }

    // item_tree() is inherited from SourceFileInfo impl above.

    fn bodies(&self) -> Option<&hir_def::bodies::CallableBodies> {
        let bodies = hir_def::def_query::callable_bodies(self.db, self.input);
        bodies.as_ref().map(|b| b.0.as_ref())
    }

    // C5: binding_type_text now queries salsa infer.
    // ide-db depends on hir-ty, so this is safe.
    fn binding_type_text(&self, span: parser::Span) -> Option<String> {
        let callable_ids = hir_def::def_query::file_def_with_body_ids(self.db, self.input);
        let bodies = self.bodies();
        for &id in callable_ids {
            let tcr = hir_ty::query::infer(self.db, id);
            if let Some(ty_text) = tcr.0.binding_type_text(span, bodies) {
                return Some(ty_text);
            }
        }
        None
    }

    fn cached_expr_type_text(&self, span: parser::Span) -> Option<String> {
        let callable_ids = hir_def::def_query::file_def_with_body_ids(self.db, self.input);
        let bodies = self.bodies();
        for &id in callable_ids {
            let tcr = hir_ty::query::infer(self.db, id);
            if let Some(ty_text) = tcr.0.expr_type_text(span, bodies) {
                return Some(ty_text);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::FileId;
    use hir_def::callgraph::SourceFileInfo;

    #[test]
    fn salsa_file_implements_file_db() {
        let db = RootDatabase::default();
        let text: Arc<str> = Arc::from("val foo : int -> int\nfunction foo(x) = x + 1\n");
        let input = FileText::new(&db, text, FileId::from_raw(0));
        let sf = SalsaFile::new(&db, input);

        // text
        assert!(sf.text().contains("foo"));

        // position_at / offset_at roundtrip
        let pos = sf.position_at(0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.col, 0);
        assert_eq!(sf.offset_at(&pos), 0);

        // tokens
        assert!(sf.tokens().is_some());

        // parsed (ParsedFile)
        let parsed = sf.parsed();
        assert!(parsed.is_some());

        // signature_index
        let idx = sf.signature_index().unwrap();
        assert!(idx.contains_key("foo"));

        // item_tree
        assert!(sf.item_tree().is_some());
    }

    #[test]
    fn salsa_file_callgraph() {
        let db = RootDatabase::default();
        let src = "function add(x : int, y : int) -> int = x + y\n\
                   function main() -> int = add(1, 2)\n";
        let input = FileText::new(&db, Arc::from(src), FileId::from_raw(0));
        let sf = SalsaFile::new(&db, input);

        let cg = sf.callgraph();
        assert!(cg.is_some());
        assert!(cg.unwrap().callees_of("main").any(|n| n == "add"), "main should call add");
    }

    #[test]
    fn salsa_file_reacts_to_text_change() {
        use salsa::Setter;
        let mut db = RootDatabase::default();
        let input = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));

        {
            let sf = SalsaFile::new(&db, input);
            let idx = sf.signature_index().unwrap();
            assert!(idx.contains_key("x"), "should have val x");
        }

        // Change text
        input.set_text(&mut db).to(Arc::from("val y : bool\n"));

        {
            let sf = SalsaFile::new(&db, input);
            let idx = sf.signature_index().unwrap();
            assert!(!idx.contains_key("x"), "x should be gone");
            assert!(idx.contains_key("y"), "should have val y");
        }
    }

    /// End-to-end integration test: use an IDE feature through the
    /// salsa-backed SalsaFile adapter. Proves the salsa query chain
    /// can serve as a drop-in replacement for the legacy File path.
    #[test]
    fn e2e_document_symbol_tree_through_salsa() {
        let db = RootDatabase::default();
        let src = "val add : (int, int) -> int\n\
                   function add(x, y) = x + y\n\
                   val sub : (int, int) -> int\n\
                   function sub(x, y) = x - y\n";
        let input = FileText::new(&db, Arc::from(src), FileId::from_raw(0));
        let sf = SalsaFile::new(&db, input);

        // document_symbol_tree is an IDE feature from ide-db that
        // takes &dyn FileDb — if this works through SalsaFile, the
        // salsa pipeline is serving real IDE features.
        let symbols = crate::symbol_index::document_symbols_ide(&sf);
        assert!(!symbols.is_empty(), "document_symbols_ide should produce targets");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "should contain 'add' symbol");
        assert!(names.contains(&"sub"), "should contain 'sub' symbol");
    }

    /// End-to-end: build_signature_index works through the legacy
    /// path using &dyn FileDb backed by salsa.
    #[test]
    fn e2e_build_signature_index_through_salsa() {
        let db = RootDatabase::default();
        let src = "val foo : int -> bool\nfunction foo(x) = true\n";
        let input = FileText::new(&db, Arc::from(src), FileId::from_raw(0));
        let sf = SalsaFile::new(&db, input);

        // Use the legacy build_signature_index (takes &dyn FileDb)
        let idx = crate::build_signature_index(&sf);
        assert!(idx.contains_key("foo"));
        let sig = &idx["foo"];
        assert_eq!(sig.return_type.as_deref(), Some("bool"));
    }
}
