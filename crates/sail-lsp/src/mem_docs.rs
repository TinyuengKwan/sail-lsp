//! In-memory document tracking for open files.
//!
//! Sail-lsp: simplified to `HashMap<Url, DocumentData>`.

use lsp_types::Url;
use std::collections::HashMap;

/// Data for a single open document.
#[derive(Clone, Debug)]
pub(crate) struct DocumentData {
    pub version: i32,
    pub data: String,
}

impl DocumentData {
    pub fn new(version: i32, data: String) -> Self {
        Self { version, data }
    }
}

/// Tracks all documents currently open in the editor.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemDocs {
    docs: HashMap<Url, DocumentData>,
    /// document set changed since last `take_changes()` call.
    added_or_removed: bool,
}

impl MemDocs {
    #[allow(dead_code)]
    pub fn contains(&self, url: &Url) -> bool {
        self.docs.contains_key(url)
    }

    pub fn insert(&mut self, url: Url, data: DocumentData) {
        self.added_or_removed = true;
        self.docs.insert(url, data);
    }

    pub fn remove(&mut self, url: &Url) {
        self.added_or_removed = true;
        self.docs.remove(url);
    }

    /// Returns true if documents were added or removed since last call.
    pub fn take_changes(&mut self) -> bool {
        std::mem::take(&mut self.added_or_removed)
    }

    #[allow(dead_code)]
    pub fn get(&self, url: &Url) -> Option<&DocumentData> {
        self.docs.get(url)
    }

    pub fn get_mut(&mut self, url: &Url) -> Option<&mut DocumentData> {
        self.docs.get_mut(url)
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &Url> {
        self.docs.keys()
    }
}
