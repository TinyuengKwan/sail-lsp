//! Bidirectional VfsPath ↔ FileId mapping.

use indexmap::IndexSet;
use rustc_hash::FxBuildHasher;

use crate::{FileId, VfsPath};

/// Maps `VfsPath` to `FileId` and back.
#[derive(Default)]
pub(crate) struct PathInterner {
    map: IndexSet<VfsPath, FxBuildHasher>,
}

impl PathInterner {
    /// Look up an existing path.
    pub(crate) fn get(&self, path: &VfsPath) -> Option<FileId> {
        self.map.get_index_of(path).map(|i| FileId::from_raw(i as u32))
    }

    /// Intern a path, returning its FileId. If already interned, returns
    /// the existing ID.
    pub(crate) fn intern(&mut self, path: VfsPath) -> FileId {
        let (idx, _) = self.map.insert_full(path);
        assert!(idx < FileId::MAX as usize);
        FileId::from_raw(idx as u32)
    }

    /// Look up the path for a FileId.
    pub(crate) fn lookup(&self, id: FileId) -> &VfsPath {
        self.map.get_index(id.index() as usize).unwrap()
    }
}
