//! # Virtual File System
//!
//! VFS records all file changes pushed to it via [`set_file_contents`].
//! It does not read files from disk; this is the responsibility of the
//! [`loader`] module.

mod anchored_path;
pub mod file_set;
pub mod loader;
mod path_interner;
mod vfs_path;

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::mem;

use indexmap::IndexMap;
use rustc_hash::FxHasher;

use crate::path_interner::PathInterner;

pub use crate::anchored_path::{AnchoredPath, AnchoredPathBuf};
pub use crate::vfs_path::VfsPath;
pub use paths::{AbsPath, AbsPathBuf};

/// Opaque handle to a file in the VFS.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct FileId(u32);

impl FileId {
    const MAX: u32 = 0x7fff_ffff;

    pub const fn from_raw(raw: u32) -> FileId {
        assert!(raw <= Self::MAX);
        FileId(raw)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}

impl From<u32> for FileId {
    #[inline]
    fn from(raw: u32) -> Self {
        FileId::from_raw(raw)
    }
}

impl nohash_hasher::IsEnabled for FileId {}

/// State of a file in the VFS.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub enum FileState {
    /// File exists with content hash.
    Exists(u64),
    /// File has been deleted.
    Deleted,
    /// File is excluded from analysis.
    Excluded,
}

/// Whether a file is excluded from analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileExcluded {
    Yes,
    No,
}

/// A changed file in the VFS.
#[derive(Debug)]
pub struct ChangedFile {
    pub file_id: FileId,
    pub change: Change,
}

impl ChangedFile {
    /// Whether the file exists after this change.
    pub fn exists(&self) -> bool {
        !matches!(self.change, Change::Delete)
    }

    /// Whether this change creates or deletes a file (structural change).
    pub fn is_created_or_deleted(&self) -> bool {
        matches!(self.change, Change::Create(..) | Change::Delete)
    }

    /// Whether this change creates a file.
    pub fn is_created(&self) -> bool {
        matches!(self.change, Change::Create(..))
    }

    /// Whether this change modifies existing content.
    pub fn is_modified(&self) -> bool {
        matches!(self.change, Change::Modify(..))
    }

    /// The kind of change.
    pub fn kind(&self) -> ChangeKind {
        match &self.change {
            Change::Create(..) => ChangeKind::Create,
            Change::Modify(..) => ChangeKind::Modify,
            Change::Delete => ChangeKind::Delete,
        }
    }
}

/// The nature of a file change.
#[derive(Eq, PartialEq, Debug)]
pub enum Change {
    /// File created (content, hash).
    Create(Vec<u8>, u64),
    /// File modified (content, hash).
    Modify(Vec<u8>, u64),
    /// File deleted.
    Delete,
}

/// Simplified change kind enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Create,
    Modify,
    Delete,
}

/// Virtual file system with content-hash dedup and change tracking.
#[derive(Default)]
pub struct Vfs {
    interner: PathInterner,
    data: Vec<FileState>,
    changes: IndexMap<FileId, ChangedFile, BuildHasherDefault<FxHasher>>,
    url_to_file_id: HashMap<url::Url, FileId>,
    file_id_to_url: HashMap<FileId, url::Url>,
}

impl std::fmt::Debug for Vfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vfs").field("n_files", &self.data.len()).finish()
    }
}

impl Vfs {
    /// Look up file ID for a path.
    pub fn file_id(&self, path: &VfsPath) -> Option<(FileId, FileExcluded)> {
        let id = self.interner.get(path)?;
        let state = self.get(id);
        match state {
            FileState::Exists(_) => Some((id, FileExcluded::No)),
            FileState::Excluded => Some((id, FileExcluded::Yes)),
            FileState::Deleted => None,
        }
    }

    /// Look up path for a file ID.
    pub fn file_path(&self, file_id: FileId) -> &VfsPath {
        self.interner.lookup(file_id)
    }

    /// Iterate all existing (file_id, path) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &VfsPath)> + '_ {
        (0..self.data.len())
            .map(|idx| FileId::from_raw(idx as u32))
            .filter(|&id| matches!(self.get(id), FileState::Exists(_)))
            .map(|id| (id, self.interner.lookup(id)))
    }

    /// Set file contents. Returns true if the VFS was actually changed.
    /// Content-hash dedup: if the hash hasn't changed, returns false.
    ///
    /// within a single cycle (Create+Modify→Create, Delete+Create→Modify, etc.).
    /// We use simpler last-write-wins since Sail's change rate is lower.
    pub fn set_file_contents(&mut self, path: VfsPath, contents: Option<Vec<u8>>) -> bool {
        let _p = tracing::span!(tracing::Level::INFO, "Vfs::set_file_contents").entered();
        let file_id = self.alloc_file_id(path);
        let old_state = self.get(file_id);
        let new_hash = contents.as_ref().map(|c| hash_content(c));

        let change = match (old_state, &contents) {
            (FileState::Deleted | FileState::Excluded, None) => return false,
            (FileState::Deleted | FileState::Excluded, Some(bytes)) => {
                let hash = new_hash.unwrap();
                self.data[file_id.index() as usize] = FileState::Exists(hash);
                Change::Create(bytes.clone(), hash)
            }
            (FileState::Exists(old_hash), Some(bytes)) => {
                let hash = new_hash.unwrap();
                if old_hash == hash {
                    return false; // Content unchanged — dedup
                }
                self.data[file_id.index() as usize] = FileState::Exists(hash);
                Change::Modify(bytes.clone(), hash)
            }
            (FileState::Exists(_), None) => {
                self.data[file_id.index() as usize] = FileState::Deleted;
                Change::Delete
            }
        };

        self.changes.insert(file_id, ChangedFile { file_id, change });
        true
    }

    /// Drain all accumulated changes.
    ///
    /// we return the same type for downstream compatibility.
    pub fn take_changes(&mut self) -> IndexMap<FileId, ChangedFile, BuildHasherDefault<FxHasher>> {
        mem::take(&mut self.changes)
    }

    /// Check if a file exists.
    pub fn exists(&self, file_id: FileId) -> bool {
        matches!(self.get(file_id), FileState::Exists(_))
    }

    /// Used by workspace loading to track files that should not be analyzed
    /// but still need a FileId for reference.
    pub fn insert_excluded_file(&mut self, path: VfsPath) {
        let id = self.alloc_file_id(path);
        if matches!(self.get(id), FileState::Deleted) {
            self.data[id.index() as usize] = FileState::Excluded;
        }
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    pub fn file_id_for_url(&mut self, url: &url::Url) -> FileId {
        if let Some(&id) = self.url_to_file_id.get(url) {
            return id;
        }
        let vfs_path = url_to_vfs_path(url);
        let id = self.alloc_file_id(vfs_path);
        self.url_to_file_id.insert(url.clone(), id);
        self.file_id_to_url.insert(id, url.clone());
        id
    }

    pub fn lookup_file_id_by_url(&self, url: &url::Url) -> Option<FileId> {
        self.url_to_file_id.get(url).copied()
    }

    pub fn lookup_url(&self, id: FileId) -> Option<&url::Url> {
        self.file_id_to_url.get(&id)
    }

    pub fn all_url_file_ids(&self) -> impl Iterator<Item = (&url::Url, FileId)> + '_ {
        self.url_to_file_id.iter().map(|(u, &id)| (u, id))
    }

    pub fn url_map_snapshot(&self) -> (HashMap<url::Url, FileId>, HashMap<FileId, url::Url>) {
        (self.url_to_file_id.clone(), self.file_id_to_url.clone())
    }

    fn alloc_file_id(&mut self, path: VfsPath) -> FileId {
        let id = self.interner.intern(path);
        while self.data.len() <= id.index() as usize {
            self.data.push(FileState::Deleted);
        }
        id
    }

    fn get(&self, file_id: FileId) -> FileState {
        self.data.get(file_id.index() as usize).copied().unwrap_or(FileState::Deleted)
    }
}

// to avoid adding stdx as a dependency (ra's vfs doesn't depend on stdx either).
fn hash_content(content: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    content.hash(&mut hasher);
    hasher.finish()
}

fn url_to_vfs_path(url: &url::Url) -> VfsPath {
    if url.scheme() == "file" {
        if let Ok(path) = url.to_file_path() {
            return VfsPath::from(paths::AbsPathBuf::assert_utf8(path));
        }
    }
    // Fall back to a platform-independent virtual path. This covers non-`file`
    // schemes and `file:` URLs that don't map to a valid native path (e.g.
    // `file:///test.sail` on Windows, which has no drive letter).
    VfsPath::new_virtual_path(format!("/{}", url.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> VfsPath {
        VfsPath::new_virtual_path(s.to_string())
    }

    #[test]
    fn set_file_contents_tracks_change() {
        let mut vfs = Vfs::default();
        let path = make_path("/test.sail");
        assert!(vfs.set_file_contents(path.clone(), Some(b"val x : int\n".to_vec())));
        assert!(vfs.has_changes());
        let changes = vfs.take_changes();
        assert_eq!(changes.len(), 1);
        assert!(changes.values().next().unwrap().is_created());
    }

    #[test]
    fn set_same_content_is_noop() {
        let mut vfs = Vfs::default();
        let path = make_path("/test.sail");
        vfs.set_file_contents(path.clone(), Some(b"val x : int\n".to_vec()));
        vfs.take_changes(); // drain

        // Set same content again — should be no-op
        assert!(!vfs.set_file_contents(path, Some(b"val x : int\n".to_vec())));
        assert!(!vfs.has_changes());
    }

    #[test]
    fn modify_content_tracks_change() {
        let mut vfs = Vfs::default();
        let path = make_path("/test.sail");
        vfs.set_file_contents(path.clone(), Some(b"val x : int\n".to_vec()));
        vfs.take_changes();

        assert!(vfs.set_file_contents(path, Some(b"val y : bool\n".to_vec())));
        let changes = vfs.take_changes();
        assert_eq!(changes.len(), 1);
        assert!(changes.values().next().unwrap().is_modified());
    }

    #[test]
    fn delete_file_tracks_change() {
        let mut vfs = Vfs::default();
        let path = make_path("/test.sail");
        vfs.set_file_contents(path.clone(), Some(b"val x : int\n".to_vec()));
        vfs.take_changes();

        assert!(vfs.set_file_contents(path, None));
        let changes = vfs.take_changes();
        assert_eq!(changes.len(), 1);
        assert!(changes.values().next().unwrap().kind() == ChangeKind::Delete);
    }

    #[test]
    fn file_id_roundtrip() {
        let mut vfs = Vfs::default();
        let path = make_path("/test.sail");
        vfs.set_file_contents(path.clone(), Some(b"hello".to_vec()));
        let (id, excluded) = vfs.file_id(&path).unwrap();
        assert_eq!(excluded, FileExcluded::No);
        assert_eq!(vfs.file_path(id), &path);
    }

    #[test]
    fn iter_only_existing() {
        let mut vfs = Vfs::default();
        let p1 = make_path("/a.sail");
        let p2 = make_path("/b.sail");
        vfs.set_file_contents(p1.clone(), Some(b"a".to_vec()));
        vfs.set_file_contents(p2.clone(), Some(b"b".to_vec()));
        vfs.take_changes();

        // Delete p1
        vfs.set_file_contents(p1, None);
        vfs.take_changes();

        let paths: Vec<_> = vfs.iter().map(|(_, p)| p.clone()).collect();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], p2);
    }

    #[test]
    fn insert_excluded_file() {
        let mut vfs = Vfs::default();
        let path = make_path("/excluded.sail");
        vfs.insert_excluded_file(path.clone());
        let result = vfs.file_id(&path);
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, FileExcluded::Yes);
        // No change should be emitted for excluded files.
        assert!(!vfs.has_changes());
    }
}
