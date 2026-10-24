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
use hir::callgraph::WorkspaceFile;
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
    pub(crate) include_instances: Arc<HashMap<base_db::preprocess::InstanceKey, base_db::FileId>>,
}

impl Drop for RootDatabase {
    fn drop(&mut self) {
        // SAFETY: storage is only dropped here, once.
        unsafe { ManuallyDrop::drop(&mut self.storage) }
    }
}

impl Default for RootDatabase {
    fn default() -> Self {
        let db = Self {
            storage: ManuallyDrop::new(salsa::Storage::default()),
            nonce: Nonce::default(),
            files: Files::default(),
            include_instances: Arc::default(),
        };
        base_db::preprocess::WorkspacePreprocessing::new(
            &db,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        );
        db
    }
}

impl Clone for RootDatabase {
    fn clone(&self) -> Self {
        Self {
            storage: ManuallyDrop::new((*self.storage).clone()),
            nonce: self.nonce.clone(),
            files: self.files.clone(),
            include_instances: self.include_instances.clone(),
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

    pub(crate) fn include_instance(
        &mut self,
        key: base_db::preprocess::InstanceKey,
        source: FileText,
    ) -> FileText {
        use salsa::Setter;
        let next = base_db::FileId::for_instance(self.include_instances.len().try_into().unwrap());
        let id = *Arc::make_mut(&mut self.include_instances).entry(key).or_insert(next);
        let text = source.text(self).clone();
        if let Some(file) = self.files.file_text(id) {
            if file.text(self) != &text {
                file.set_text(self).to(text);
            }
            file
        } else {
            let file = FileText::new(self, text, id);
            self.files.register_instance(id, file);
            file
        }
    }

    // There is no `files_mut`. Every write reaches the store through
    // `apply_change` — `durability_is_decided_in_one_place` is the rule
    // and this was the second door, open and unused.
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
        // A write that changes nothing still bumps a revision and makes every
        // dependent query re-verify, so it is dropped here — in the store that
        // owns the hashes, rather than in a buffer one of three callers
        // remembered to use.
        if !self.files.content_changed(file_id, text) {
            return;
        }
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

    fn source_root(&self, id: base_db::SourceRootId) -> base_db::SourceRootInput {
        self.files
            .source_root(id)
            .unwrap_or_else(|| panic!("source_root: unknown SourceRootId {id:?}"))
    }

    fn file_source_root(&self, id: base_db::FileId) -> base_db::FileSourceRootInput {
        self.files
            .file_source_root(id)
            .unwrap_or_else(|| panic!("file_source_root: unclassified FileId {id:?}"))
    }

    fn source_root_id_of(&self, file_id: base_db::FileId) -> Option<base_db::SourceRootId> {
        let input = self.files.file_source_root(file_id)?;
        Some(input.source_root_id(self))
    }

    fn source_root_data(&self, id: base_db::SourceRootId) -> Option<Arc<base_db::SourceRoot>> {
        Some(self.files.source_root(id)?.source_root(self))
    }

    fn set_file_source_root_with_durability(
        &mut self,
        id: base_db::FileId,
        source_root_id: base_db::SourceRootId,
        durability: base_db::Durability,
    ) {
        let existing = self.files.file_source_root(id);
        if let Some(input) = existing {
            use salsa::Setter;
            input.set_source_root_id(self).with_durability(durability).to(source_root_id);
        } else {
            let input = base_db::FileSourceRootInput::builder(source_root_id)
                .durability(durability)
                .new(self);
            self.files.register_file_source_root(id, input);
        }
    }

    fn set_source_root_with_durability(
        &mut self,
        source_root_id: base_db::SourceRootId,
        source_root: Arc<base_db::SourceRoot>,
        durability: base_db::Durability,
    ) {
        let existing = self.files.source_root(source_root_id);
        if let Some(input) = existing {
            use salsa::Setter;
            input.set_source_root(self).with_durability(durability).to(source_root);
        } else {
            let input =
                base_db::SourceRootInput::builder(source_root).durability(durability).new(self);
            self.files.register_source_root(source_root_id, input);
        }
    }
}
// `ExpandDatabase`, `DefDatabase` and `HirDatabase` were implemented here
// — eight methods forwarding to the tracked functions — and never called.
// rust-analyzer's versions are the interface: `db.body(def)`, `db.infer(def)`,
// hundreds of methods, everywhere. Ours are `#[salsa::tracked]` free
// functions, which is the same thing without the trait, so the traits
// mirrored RA's names over nothing. Emptying them broke only these impls.

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

    fn callgraph(&self) -> Option<&hir::callgraph::CallGraph> {
        hir::def_query::callgraph(self.db, self.input).as_ref().map(|acg| acg.0.as_ref())
    }
}

impl<'a> hir::callgraph::SourceFileInfo for SalsaFile<'a> {
    fn text(&self) -> &str {
        self.input.text(self.db).as_ref()
    }

    fn item_tree(&self) -> Option<&hir::ItemTree> {
        let it = hir::def_query::file_item_tree(self.db, self.input);
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
        // The token containing the offset, or — if none does — the one that
        // ends there. A cursor immediately after a name is where an editor
        // leaves it once the name is typed, and where a double-click then a
        // goto often lands; a half-open test alone answered nothing at all
        // for it. The containing token still wins, so no existing answer
        // changes.
        tokens
            .iter()
            .rev()
            .find(|(_, span)| span.start <= offset && offset < span.end)
            .or_else(|| tokens.iter().rev().find(|(_, span)| span.end == offset))
    }

    fn file_text(&self) -> Option<(&dyn salsa::Database, base_db::FileText)> {
        Some((self.db, self.input))
    }

    fn syntax(&self) -> Option<syntax::SyntaxNode> {
        let parsed = syntax::parse_query::parse_file(self.db, self.input);
        parsed.green.as_ref().map(|g| syntax::SyntaxNode::new_root((**g).clone()))
    }

    fn parsed(&self) -> Option<&ParsedFile> {
        // Enriched with the local bindings the CST lowering does not record;
        // hover and find-references resolve locals through `decls`.
        crate::db_query::enriched_parsed_file(self.db, self.input).as_ref().map(Arc::as_ref)
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

    fn bodies(&self) -> Option<&hir::CallableBodies> {
        let bodies = hir::def_query::callable_bodies(self.db, self.input);
        bodies.as_ref().map(|b| b.0.as_ref())
    }

    // C5: binding_type_text now queries salsa infer.
    // ide-db depends on hir-ty, so this is safe.
    fn binding_type_text(&self, span: parser::Span) -> Option<String> {
        let callable_ids = hir::def_query::file_def_with_body_ids(self.db, self.input);
        let bodies = self.bodies();
        for &id in callable_ids {
            let tcr = hir::ty_query::infer(self.db, id);
            if let Some(ty_text) = tcr.0.binding_type_text(span, bodies) {
                return Some(ty_text);
            }
        }
        None
    }

    fn cached_expr_type_text(&self, span: parser::Span) -> Option<String> {
        let callable_ids = hir::def_query::file_def_with_body_ids(self.db, self.input);
        let bodies = self.bodies();
        for &id in callable_ids {
            let tcr = hir::ty_query::infer(self.db, id);
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
    use hir::callgraph::SourceFileInfo;

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
