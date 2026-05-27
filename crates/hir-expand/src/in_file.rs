//! `InFile<T>` stores a value of `T` inside a particular file.
//! Typical usages:
//!
//! * `InFile<SyntaxNode>` -- syntax node in a file
//! * `InFile<SyntaxNodePtr>` -- stable pointer in a file
//! * `InFile<TextSize>` -- offset in a file
//!
//! # Simplification vs RA
//!
//! RA has `InFileWrapper<FileKind, T>` generic over file kind because
//! macros introduce `HirFileId` / `MacroCallId` / `EditionedFileId`.
//! Sail has no macros, so we use a concrete `FileId` directly.

use base_db::FileId;

/// `InFile<T>` stores a value of `T` inside a particular file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InFile<T> {
    pub file_id: FileId,
    pub value: T,
}

impl<T> InFile<T> {
    pub fn new(file_id: FileId, value: T) -> Self {
        InFile { file_id, value }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> InFile<U> {
        InFile { file_id: self.file_id, value: f(self.value) }
    }

    pub fn as_ref(&self) -> InFile<&T> {
        InFile { file_id: self.file_id, value: &self.value }
    }

    pub fn with_value<U>(&self, value: U) -> InFile<U> {
        InFile { file_id: self.file_id, value }
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }
}

/// A position (file + offset) in the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePosition {
    pub file_id: FileId,
    pub offset: rowan::TextSize,
}

/// A range (file + text range) in the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileRange {
    pub file_id: FileId,
    pub range: rowan::TextRange,
}

/// Allow converting a bare `TextRange` into a `FileRange` with a placeholder
/// `FileId::from_raw(0)`. This supports incremental migration: callers that lack a real
/// file id (parse-error pipeline, from_proto round-trip) can pass a plain
/// `TextRange` and the file id will be patched later by the caller that owns it.
impl From<rowan::TextRange> for FileRange {
    fn from(range: rowan::TextRange) -> Self {
        FileRange { file_id: FileId::from_raw(0), range }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_file_new_and_accessors() {
        let fid = FileId::from_raw(42);
        let inf = InFile::new(fid, "hello");
        assert_eq!(inf.file_id(), fid);
        assert_eq!(inf.value, "hello");
    }

    #[test]
    fn in_file_map() {
        let inf = InFile::new(FileId::from_raw(1), 10u32);
        let mapped = inf.map(|v| v * 2);
        assert_eq!(mapped.value, 20);
        assert_eq!(mapped.file_id, FileId::from_raw(1));
    }

    #[test]
    fn in_file_with_value() {
        let inf = InFile::new(FileId::from_raw(1), "a");
        let replaced = inf.with_value(42);
        assert_eq!(replaced.file_id, FileId::from_raw(1));
        assert_eq!(replaced.value, 42);
    }

    #[test]
    fn in_file_as_ref() {
        let inf = InFile::new(FileId::from_raw(1), String::from("hi"));
        let r = inf.as_ref();
        assert_eq!(r.value, "hi");
    }

    #[test]
    fn file_position_eq() {
        let a = FilePosition { file_id: FileId::from_raw(1), offset: rowan::TextSize::from(10) };
        let b = FilePosition { file_id: FileId::from_raw(1), offset: rowan::TextSize::from(10) };
        assert_eq!(a, b);
    }

    #[test]
    fn file_range_eq() {
        let r = rowan::TextRange::new(rowan::TextSize::from(0), rowan::TextSize::from(5));
        let a = FileRange { file_id: FileId::from_raw(1), range: r };
        let b = FileRange { file_id: FileId::from_raw(1), range: r };
        assert_eq!(a, b);
    }
}
