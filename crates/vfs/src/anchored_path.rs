//! Anchored path types for file resolution relative to another file.

use crate::FileId;

/// An owned path anchored to a file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AnchoredPathBuf {
    /// The file this path is relative to.
    pub anchor: FileId,
    /// The relative path string.
    pub path: String,
}

/// A borrowed path anchored to a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AnchoredPath<'a> {
    /// The file this path is relative to.
    pub anchor: FileId,
    /// The relative path string.
    pub path: &'a str,
}
