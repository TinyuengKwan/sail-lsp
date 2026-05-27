//! Source root and file-to-root mapping types.
//!
//! Sail is a flat, single-"crate" language. ra's `CrateGraphBuilder`
//! (~900 lines), `Crate`, `CrateData`, `CrateName`, `CrateOrigin`,
//! `Env`, `ProcMacroPaths` are intentionally absent.

use std::sync::Arc;

use ::vfs::file_set::FileSet;
use ::vfs::{AnchoredPath, FileId, VfsPath};

pub use salsa::Durability;

/// Identifies a source root (a group of files with shared properties).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceRootId(pub u32);

/// A set of files forming a logical unit.
///
/// `file_set()` which ra's SourceRoot doesn't expose (ra mutates only inside
/// CrateGraphBuilder / FileChange::apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRoot {
    /// Whether this root is a library (stable, rarely modified).
    pub is_library: bool,
    file_set: FileSet,
}

impl SourceRoot {
    /// Create a local (non-library) source root.
    pub fn new_local(file_set: FileSet) -> Self {
        Self { is_library: false, file_set }
    }

    /// Create a library source root (stable, HIGH durability).
    pub fn new_library(file_set: FileSet) -> Self {
        Self { is_library: true, file_set }
    }

    /// Whether this root contains the given file.
    pub fn contains(&self, file: FileId) -> bool {
        self.file_set.path_for_file(&file).is_some()
    }

    /// Iterate all file IDs in this root.
    pub fn iter(&self) -> impl Iterator<Item = FileId> + '_ {
        self.file_set.iter()
    }

    pub fn len(&self) -> usize {
        self.file_set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file_set.is_empty()
    }

    /// Insert a file with its path.
    pub fn insert(&mut self, file: FileId, path: VfsPath) {
        self.file_set.insert(file, path);
    }

    /// Look up path for a file ID.
    pub fn path_for_file(&self, file: &FileId) -> Option<&VfsPath> {
        self.file_set.path_for_file(file)
    }

    /// Look up file ID by path.
    pub fn file_for_path(&self, path: &VfsPath) -> Option<&FileId> {
        self.file_set.file_for_path(path)
    }

    /// Resolve a relative path anchored to a file in this root.
    pub fn resolve_path(&self, path: AnchoredPath<'_>) -> Option<FileId> {
        self.file_set.resolve_path(path)
    }

    /// All files in this source root.
    pub fn file_set(&self) -> &FileSet {
        &self.file_set
    }
}

/// Salsa input wrapping `Arc<SourceRoot>`.
#[salsa::input(debug)]
pub struct SourceRootInput {
    pub source_root: Arc<SourceRoot>,
}

/// Salsa input mapping a file to its source root.
#[salsa::input(debug)]
pub struct FileSourceRootInput {
    pub source_root_id: SourceRootId,
}

/// Durability for source root metadata.
pub fn source_root_durability(source_root: &SourceRoot) -> Durability {
    if source_root.is_library {
        Durability::MEDIUM
    } else {
        Durability::LOW
    }
}

/// Durability for file text within a source root.
pub fn file_text_durability(source_root: &SourceRoot) -> Durability {
    if source_root.is_library {
        Durability::HIGH
    } else {
        Durability::LOW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> VfsPath {
        VfsPath::new(paths::AbsPathBuf::assert(paths::Utf8PathBuf::from(s)))
    }

    #[test]
    fn source_root_local() {
        let mut file_set = FileSet::default();
        file_set.insert(FileId::from_raw(0), make_path("/a.sail"));
        file_set.insert(FileId::from_raw(1), make_path("/b.sail"));
        file_set.insert(FileId::from_raw(2), make_path("/c.sail"));
        let root = SourceRoot::new_local(file_set);
        assert!(!root.is_library);
        assert_eq!(root.len(), 3);
        assert!(root.contains(FileId::from_raw(0)));
        assert!(!root.contains(FileId::from_raw(99)));
    }

    #[test]
    fn source_root_library() {
        let mut file_set = FileSet::default();
        file_set.insert(FileId::from_raw(10), make_path("/lib.sail"));
        let root = SourceRoot::new_library(file_set);
        assert!(root.is_library);
        assert_eq!(root.len(), 1);
        assert!(root.contains(FileId::from_raw(10)));
    }

    #[test]
    fn source_root_mutate() {
        let mut root = SourceRoot::new_local(FileSet::default());
        assert!(root.is_empty());
        root.insert(FileId::from_raw(5), make_path("/x.sail"));
        assert_eq!(root.len(), 1);
        assert!(root.contains(FileId::from_raw(5)));
    }

    #[test]
    fn durability_levels() {
        let local = SourceRoot::new_local(FileSet::default());
        let library = SourceRoot::new_library(FileSet::default());

        assert_eq!(source_root_durability(&local), Durability::LOW);
        assert_eq!(source_root_durability(&library), Durability::MEDIUM);
        assert_eq!(file_text_durability(&local), Durability::LOW);
        assert_eq!(file_text_durability(&library), Durability::HIGH);
    }
}
