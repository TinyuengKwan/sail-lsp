//! Salsa-backed source database for sail-lsp.
//!
//! - No CrateGraph / Edition — Sail uses flat workspace model (CUSTOM)
//! - WorkspaceFiles / WorkspaceFixities salsa singletons replace AllCrates (CUSTOM)
//! - Files manages only FileText; ra's Files manages all three input kinds
//! - Span/TextRange bridge helpers for Sail's usize-based parser (CUSTOM)

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use dashmap::DashMap;
use rustc_hash::FxHasher;
pub use salsa::Durability;

mod change;
pub mod input;

pub use change::FileChange;
pub use input::{
    file_text_durability, source_root_durability, FileSourceRootInput, SourceRoot, SourceRootId,
    SourceRootInput,
};

/// Re-export rowan's TextRange and TextSize directly.
pub use rowan::{TextRange, TextSize};

// Sail's parser uses usize-based Span, so we need converters.

/// Construct a `TextRange` from usize byte offsets.
#[inline]
pub fn text_range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

/// Get the start offset of a TextRange as usize.
#[inline]
pub fn range_start(range: TextRange) -> usize {
    u32::from(range.start()) as usize
}

/// Get the end offset of a TextRange as usize.
#[inline]
pub fn range_end(range: TextRange) -> usize {
    u32::from(range.end()) as usize
}

/// Get the length of a TextRange as usize.
#[inline]
pub fn range_len(range: TextRange) -> usize {
    range.len().into()
}

/// Convert a `parser::Span` (usize offsets) to a `rowan::TextRange`.
#[inline]
pub fn span_to_text_range(span: &parser::Span) -> TextRange {
    TextRange::new(TextSize::from(span.start as u32), TextSize::from(span.end as u32))
}

/// Convert a `rowan::TextRange` back to a `parser::Span`.
#[inline]
pub fn text_range_to_span(range: TextRange) -> parser::Span {
    parser::Span { start: u32::from(range.start()) as usize, end: u32::from(range.end()) as usize }
}

/// Handle to a file in the virtual file system.
pub use ::vfs::FileId;

/// VFS path type.
pub use ::vfs::VfsPath;

/// VFS change types.
pub use ::vfs::{Change as VfsChange, ChangeKind, ChangedFile};

/// File exclusion marker.
pub use ::vfs::FileExcluded;

/// File set for source root partitioning.
pub use ::vfs::file_set::{FileSet, FileSetConfig, FileSetConfigBuilder};

/// Anchored path for relative file resolution.
pub use ::vfs::{AnchoredPath, AnchoredPathBuf};


/// Salsa input: the source text for a single file.
#[salsa::input(debug)]
pub struct FileText {
    #[returns(ref)]
    pub text: Arc<str>,
    pub file_id: FileId,
}

/// Manages the set of files known to the database.
///
/// file_source_roots) and uses `&self` (interior mutability). Our `Files` manages
/// only `FileText` entries; source roots are handled via `SourceDatabase` trait.
/// We use `&mut self` for writes (pending-change pattern avoids salsa borrow conflicts).
#[derive(Clone)]
pub struct Files {
    /// FileId → FileText salsa input. `Arc<DashMap>` for cheap clone + concurrent read.
    map: Arc<DashMap<FileId, FileText>>,
    /// Content-hash per file for dedup.
    content_hashes: HashMap<FileId, u64>,
    /// Pending changes buffered by set_file_contents().
    /// Flushed to salsa by apply_pending().
    pending: Vec<(FileId, Arc<str>, Durability)>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            map: Arc::new(DashMap::new()),
            content_hashes: HashMap::default(),
            pending: Vec::new(),
        }
    }
}

impl Files {
    /// Buffer a file change without writing to salsa.
    /// Analogous to `Vfs::set_file_contents()` — stores in pending queue.
    /// Call `apply_pending(db)` to flush all buffered changes to salsa.
    ///
    /// The caller is responsible for obtaining the FileId from `vfs::Vfs`
    /// (URL ↔ FileId mapping now lives in the VFS layer).
    pub fn set_file_contents(&mut self, file_id: FileId, text: &str, durability: Durability) {
        // Content-hash dedup before buffering
        let hash = {
            let mut h = FxHasher::default();
            text.hash(&mut h);
            h.finish()
        };
        if self.content_hashes.get(&file_id) == Some(&hash) {
            return; // Content unchanged — don't buffer
        }
        self.content_hashes.insert(file_id, hash);
        self.pending.push((file_id, Arc::from(text), durability));
    }

    /// Flush pending changes to salsa in a single batch.
    /// Analogous to `process_changes()` → `analysis_host.apply_change()`.
    /// Returns true if any changes were applied.
    pub fn apply_pending(&mut self, db: &mut dyn salsa::Database) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let changes: Vec<_> = self.pending.drain(..).collect();
        for (file_id, text, durability) in changes {
            use salsa::Setter;
            if let Some(existing) = self.map.get(&file_id) {
                existing.set_text(db).with_durability(durability).to(text);
            } else {
                let ft = FileText::builder(text, file_id).durability(durability).new(db);
                self.map.insert(file_id, ft);
            }
        }
        true
    }

    /// Check if there are pending changes.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drain all pending changes into a Vec, leaving the pending queue empty.
    ///
    /// Used by `RootDatabase::apply_pending_file_changes` to solve the
    /// borrow-conflict (Files + db are the same object).
    pub fn take_pending(&mut self) -> Vec<(FileId, Arc<str>, Durability)> {
        std::mem::take(&mut self.pending)
    }

    /// Set file text with explicit durability. Open (edited) files get
    /// `Durability::LOW`; stable library/disk files get `Durability::HIGH`.
    pub fn set_file_text_with_durability(
        &mut self,
        db: &mut dyn salsa::Database,
        file_id: FileId,
        text: &str,
        durability: Durability,
    ) {
        // Content-hash dedup: skip salsa write if text unchanged.
        let hash = {
            let mut h = FxHasher::default();
            text.hash(&mut h);
            h.finish()
        };
        if self.content_hashes.get(&file_id) == Some(&hash) {
            return; // Content unchanged — no-op
        }
        self.content_hashes.insert(file_id, hash);

        use salsa::Setter;
        let arc_text: Arc<str> = Arc::from(text);
        if let Some(existing) = self.map.get(&file_id) {
            existing.set_text(db).with_durability(durability).to(arc_text);
        } else {
            let ft = FileText::builder(arc_text, file_id).durability(durability).new(db);
            self.map.insert(file_id, ft);
        }
    }

    /// Set file text with default durability (`LOW` — suitable for open files).
    pub fn set_file_text(&mut self, db: &mut dyn salsa::Database, file_id: FileId, text: &str) {
        self.set_file_text_with_durability(db, file_id, text, Durability::LOW);
    }

    /// Register an already-created `FileText` salsa input in the map.
    ///
    /// Used by `SourceDatabase` impl where the FileText is created
    /// directly via salsa (avoiding the borrow-conflict with `set_file_text`).
    pub fn register(&mut self, file_id: FileId, ft: FileText) {
        self.map.insert(file_id, ft);
    }

    /// Look up the `FileText` salsa input for a given `FileId`.
    pub fn file_text(&self, file_id: FileId) -> Option<FileText> {
        self.map.get(&file_id).map(|r| *r.value())
    }

    /// Iterate all known file IDs.
    pub fn all_file_ids(&self) -> Vec<FileId> {
        self.map.iter().map(|r| *r.key()).collect()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// the workspace is a flat list of files.

/// Salsa input: the set of all files in the workspace.
#[salsa::input(singleton)]
pub struct WorkspaceFiles {
    #[returns(ref)]
    pub file_texts: Vec<FileText>,
}

// whose binding powers must be shared across the entire parse phase.

/// Workspace-wide operator fixity declarations.
#[salsa::input(singleton)]
pub struct WorkspaceFixities {
    /// Content-hash fingerprint for change detection.
    pub fingerprint: u64,
    /// Merged fixity context from all workspace files.
    #[returns(ref)]
    pub fixities: std::collections::HashMap<String, (u8, u8)>,
}

/// The base database trait that all higher-level crates depend on.
///
/// We use a plain trait since salsa 0.25 doesn't require trait-level macros.
pub trait SourceDatabase: salsa::Database {
    /// Get the FileText salsa input for a file.
    ///
    /// Callers use `.text(db)` to get the actual string content.
    ///
    /// # Panics
    /// Panics if `file_id` has not been registered via `set_file_text`.
    /// All queried FileIds must exist.
    fn file_text(&self, file_id: FileId) -> FileText;

    /// All file IDs currently known.
    fn all_file_ids(&self) -> Vec<FileId>;

    /// Set file text with default durability (LOW).
    fn set_file_text(&mut self, file_id: FileId, text: &str);

    /// Set file text with explicit durability.
    fn set_file_text_with_durability(
        &mut self,
        file_id: FileId,
        text: &str,
        durability: Durability,
    );

    /// Get the source root for a given `SourceRootId`.
    fn source_root(&self, id: SourceRootId) -> SourceRootInput;

    /// Get the source root that a file belongs to.
    fn file_source_root(&self, id: FileId) -> FileSourceRootInput;

    /// Set which source root a file belongs to, with explicit durability.
    fn set_file_source_root_with_durability(
        &mut self,
        id: FileId,
        source_root_id: SourceRootId,
        durability: Durability,
    );

    /// Set a source root's content, with explicit durability.
    fn set_source_root_with_durability(
        &mut self,
        source_root_id: SourceRootId,
        source_root: Arc<SourceRoot>,
        durability: Durability,
    );

    /// Default implementation looks up the anchor's source root and delegates.
    fn resolve_path(&self, path: AnchoredPath<'_>) -> Option<FileId> {
        let source_root_input = self.file_source_root(path.anchor);
        let source_root = self.source_root(source_root_input.source_root_id(self));
        source_root.source_root(self).resolve_path(path)
    }

    /// Nonce identifies the database instance; Revision tracks the current
    /// mutation version. Used for GC gating and thread-local cache invalidation.
    fn nonce_and_revision(&self) -> (Nonce, salsa::Revision) {
        // Default: fetch via salsa plumbing
        let revision = salsa::plumbing::ZalsaDatabase::zalsa(self).current_revision();
        // Nonce is per-database-instance; default returns a fresh one each time.
        // Concrete implementors should store a Nonce field and return it.
        (Nonce::new(), revision)
    }
}

static NEXT_NONCE: AtomicUsize = AtomicUsize::new(0);

/// Unique identifier for a database instance.
///
/// detect when a thread-local cache is stale (different db instance or
/// different revision).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Nonce(usize);

impl Default for Nonce {
    #[inline]
    fn default() -> Self {
        Nonce::new()
    }
}

impl Nonce {
    #[inline]
    pub fn new() -> Nonce {
        Nonce(NEXT_NONCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: Files,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    impl SourceDatabase for TestDb {
        fn file_text(&self, file_id: FileId) -> FileText {
            self.files
                .file_text(file_id)
                .unwrap_or_else(|| panic!("file_text: unknown FileId {:?}", file_id))
        }
        fn all_file_ids(&self) -> Vec<FileId> {
            self.files.all_file_ids()
        }
        fn set_file_text(&mut self, file_id: FileId, text: &str) {
            // Split borrow: check if FileText exists, then operate on self
            let existing = self.files.file_text(file_id);
            if let Some(ft) = existing {
                ft.set_text(self).to(Arc::from(text));
            } else {
                let ft = FileText::new(self, Arc::from(text), file_id);
                self.files.register(file_id, ft);
            }
        }
        fn set_file_text_with_durability(
            &mut self,
            file_id: FileId,
            text: &str,
            durability: Durability,
        ) {
            let existing = self.files.file_text(file_id);
            if let Some(ft) = existing {
                ft.set_text(self).with_durability(durability).to(Arc::from(text));
            } else {
                let ft = FileText::new(self, Arc::from(text), file_id);
                self.files.register(file_id, ft);
            }
        }
        fn source_root(&self, _id: SourceRootId) -> SourceRootInput {
            unimplemented!()
        }
        fn file_source_root(&self, _id: FileId) -> FileSourceRootInput {
            unimplemented!()
        }
        fn set_file_source_root_with_durability(
            &mut self,
            _: FileId,
            _: SourceRootId,
            _: Durability,
        ) {
            unimplemented!()
        }
        fn set_source_root_with_durability(
            &mut self,
            _: SourceRootId,
            _: Arc<SourceRoot>,
            _: Durability,
        ) {
            unimplemented!()
        }
    }

    #[test]
    fn file_text_roundtrip() {
        let db = TestDb::default();
        let text: Arc<str> = Arc::from("val x : int\n");
        let file_id = FileId::from_raw(0);
        let ft = FileText::new(&db, text.clone(), file_id);

        assert_eq!(ft.text(&db).as_ref(), "val x : int\n");
        assert_eq!(ft.file_id(&db), file_id);
    }

    #[test]
    fn file_text_update() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let ft = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));
        assert_eq!(ft.text(&db).as_ref(), "val x : int\n");

        ft.set_text(&mut db).to(Arc::from("val x : bool\n"));
        assert_eq!(ft.text(&db).as_ref(), "val x : bool\n");
    }

    #[test]
    fn files_collection() {
        let mut db = TestDb::default();
        let mut files = Files::default();

        let id0 = FileId::from_raw(0);
        let id1 = FileId::from_raw(1);
        files.set_file_text(&mut db, id0, "val x : int\n");
        files.set_file_text(&mut db, id1, "val y : bool\n");

        let ft0 = files.file_text(id0).unwrap();
        assert_eq!(ft0.text(&db).as_ref(), "val x : int\n");

        files.set_file_text(&mut db, id0, "val x : bool\n");
        assert_eq!(ft0.text(&db).as_ref(), "val x : bool\n");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn durability_levels() {
        let mut db = TestDb::default();
        let mut files = Files::default();

        let id_lib = FileId::from_raw(0);
        let id_local = FileId::from_raw(1);

        // Library file: HIGH durability (stable)
        files.set_file_text_with_durability(
            &mut db,
            id_lib,
            "val lib_fn : int\n",
            Durability::HIGH,
        );
        // Local file: LOW durability (frequently edited)
        files.set_file_text_with_durability(
            &mut db,
            id_local,
            "val local_fn : bool\n",
            Durability::LOW,
        );

        // Both should be readable
        let ft_lib = files.file_text(id_lib).unwrap();
        let ft_local = files.file_text(id_local).unwrap();
        assert_eq!(ft_lib.text(&db).as_ref(), "val lib_fn : int\n");
        assert_eq!(ft_local.text(&db).as_ref(), "val local_fn : bool\n");

        // Update local file (LOW) — should not affect lib file (HIGH)
        files.set_file_text_with_durability(
            &mut db,
            id_local,
            "val local_fn : int\n",
            Durability::LOW,
        );
        assert_eq!(ft_local.text(&db).as_ref(), "val local_fn : int\n");
        assert_eq!(ft_lib.text(&db).as_ref(), "val lib_fn : int\n");
    }

    #[test]
    fn file_change_batch() {
        let mut db = TestDb::default();

        let id0 = FileId::from_raw(0);
        let id1 = FileId::from_raw(1);

        // Batch two file creations
        let mut change = FileChange::new();
        change.change_file(id0, Some("val x : int\n".to_string()));
        change.change_file(id1, Some("val y : bool\n".to_string()));
        change.apply(&mut db);

        assert_eq!(db.files.file_text(id0).unwrap().text(&db).as_ref(), "val x : int\n");
        assert_eq!(db.files.file_text(id1).unwrap().text(&db).as_ref(), "val y : bool\n");

        // Batch an update
        let mut change2 = FileChange::new();
        change2.change_file(id0, Some("val x : bool\n".to_string()));
        change2.apply(&mut db);
        assert_eq!(db.files.file_text(id0).unwrap().text(&db).as_ref(), "val x : bool\n");
    }

}
