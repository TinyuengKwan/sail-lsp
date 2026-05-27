//! Defines a unit of change that can be applied to the database.
//!
//! returns `Option<CratesIdMap>` from apply(). We omit the crate graph field
//! since Sail has no multi-crate model (CUSTOM).
//!
//! All setter calls happen within a single `&mut db` borrow so salsa
//! treats them as one atomic revision bump.

use std::sync::Arc;

use crate::input::SourceRoot;
use ::vfs::FileId;

/// A batch of file changes to apply atomically to the database.
///
/// Accumulate changes, then call `apply()` to commit them all.
#[derive(Default)]
pub struct FileChange {
    /// New source roots to replace the current set.
    pub roots: Option<Vec<SourceRoot>>,
    /// Individual file text changes.
    pub files_changed: Vec<(FileId, Option<String>)>,
}

impl std::fmt::Debug for FileChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("FileChange");
        if let Some(roots) = &self.roots {
            d.field("roots", &roots.len());
        }
        if !self.files_changed.is_empty() {
            d.field("files_changed", &self.files_changed.len());
        }
        d.finish()
    }
}

impl FileChange {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the new source roots.
    pub fn set_roots(&mut self, roots: Vec<SourceRoot>) {
        self.roots = Some(roots);
    }

    /// Queue a file text change. `None` means the file was deleted.
    pub fn change_file(&mut self, file_id: FileId, new_text: Option<String>) {
        self.files_changed.push((file_id, new_text));
    }

    /// Apply all queued changes to the database.
    ///
    /// All file text and source root changes are applied atomically
    /// within a single salsa revision bump.
    pub fn apply(self, db: &mut dyn crate::SourceDatabase) {
        // Apply source root changes inline.
        if let Some(roots) = self.roots {
            for (idx, root) in roots.into_iter().enumerate() {
                let durability = crate::input::source_root_durability(&root);
                db.set_source_root_with_durability(
                    crate::input::SourceRootId(idx as u32),
                    Arc::new(root),
                    durability,
                );
            }
        }
        // Apply file text changes.
        for (file_id, text) in self.files_changed {
            let text = text.unwrap_or_default();
            db.set_file_text(file_id, &text);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_none() && self.files_changed.is_empty()
    }
}
