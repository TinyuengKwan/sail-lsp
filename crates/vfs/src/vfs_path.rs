//! VfsPath — abstract path type for the VFS.
//!
//! We keep the single-variant representation since Sail has no virtual/in-memory files,
//! but align the public API signatures (as_path -> Option, strip_prefix, etc.).

use paths::{AbsPath, AbsPathBuf, RelPath};

/// Path type for the VFS.
///
/// The public API is aligned: `as_path()` returns `Option` (always `Some` for us),
/// `strip_prefix` is available, `encode` is `pub(crate)`.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct VfsPath(AbsPathBuf);

impl VfsPath {
    pub fn new(path: AbsPathBuf) -> Self {
        VfsPath(path)
    }

    // TODO(align): ra has `new_virtual_path(path: String) -> VfsPath` for in-memory test paths.
    // Sail doesn't need virtual paths; add if test infrastructure requires it.

    // TODO(align): ra has `new_real_path(path: String) -> VfsPath` convenience constructor.

    /// For sail-lsp this is always `Some` since we only have real paths.
    pub fn as_path(&self) -> Option<&AbsPath> {
        Some(&self.0)
    }

    pub fn into_abs_path(self) -> Option<AbsPathBuf> {
        Some(self.0)
    }

    /// Join a relative path onto this VfsPath.
    pub fn join(&self, path: &str) -> Option<VfsPath> {
        Some(VfsPath(self.0.join(path)))
    }

    /// Remove the last component of this path, returning true if successful.
    pub fn pop(&mut self) -> bool {
        self.0.pop()
    }

    /// Check if this path starts with the given prefix.
    pub fn starts_with(&self, other: &VfsPath) -> bool {
        self.0.starts_with(&other.0)
    }

    pub fn strip_prefix(&self, other: &VfsPath) -> Option<&RelPath> {
        self.0.strip_prefix(&other.0)
    }

    /// Returns the `VfsPath` without its final component, if there is one.
    pub fn parent(&self) -> Option<VfsPath> {
        let mut parent = self.clone();
        if parent.pop() {
            Some(parent)
        } else {
            None
        }
    }

    /// Returns `self`'s base name and file extension.
    pub fn name_and_extension(&self) -> Option<(&str, Option<&str>)> {
        self.0.name_and_extension()
    }

    /// Encode the path into the given buffer (for prefix matching).
    #[allow(dead_code)] // kept for API alignment with ra
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        buf.push(0); // tag: real path (ra tag 0 = PathBuf, 1 = VirtualPath)
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            buf.extend(self.0.as_os_str().as_bytes());
        }
        #[cfg(not(unix))]
        {
            // TODO(align): ra uses UTF-16 LE wide char encoding on Windows
            // for case/separator-agnostic FST keys.
            buf.extend(self.0.as_os_str().to_string_lossy().as_bytes());
        }
    }
}

impl From<AbsPathBuf> for VfsPath {
    fn from(path: AbsPathBuf) -> Self {
        VfsPath(path.normalize())
    }
}

impl PartialEq<AbsPath> for VfsPath {
    fn eq(&self, other: &AbsPath) -> bool {
        self.0.as_path() == other
    }
}

impl PartialEq<VfsPath> for AbsPath {
    fn eq(&self, other: &VfsPath) -> bool {
        self == other.0.as_path()
    }
}

impl std::fmt::Debug for VfsPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::fmt::Display for VfsPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_produces_tagged_bytes() {
        let path = VfsPath::new(AbsPathBuf::assert_utf8("/tmp/test.sail".into()));
        let mut buf = Vec::new();
        path.encode(&mut buf);
        // First byte is the tag (0 = real path).
        assert_eq!(buf[0], 0);
        // Remaining bytes are the OS path representation.
        assert!(buf.len() > 1);
        assert!(buf[1..].ends_with(b"/tmp/test.sail"));
    }

    #[test]
    fn as_path_returns_some() {
        let path = VfsPath::new(AbsPathBuf::assert_utf8("/tmp/test.sail".into()));
        assert!(path.as_path().is_some());
    }

    #[test]
    fn from_normalizes() {
        let path = VfsPath::from(AbsPathBuf::assert_utf8("/tmp/../tmp/test.sail".into()));
        assert_eq!(path.as_path().unwrap().as_str(), "/tmp/test.sail");
    }

    #[test]
    fn strip_prefix_works() {
        let base = VfsPath::new(AbsPathBuf::assert_utf8("/project".into()));
        let child = VfsPath::new(AbsPathBuf::assert_utf8("/project/src/main.sail".into()));
        let rel = child.strip_prefix(&base);
        assert!(rel.is_some());
        assert_eq!(rel.unwrap().as_str(), "src/main.sail");
    }

    #[test]
    fn cross_type_eq() {
        let vfs = VfsPath::new(AbsPathBuf::assert_utf8("/tmp/test.sail".into()));
        let abs = AbsPath::assert("/tmp/test.sail".into());
        assert_eq!(vfs, *abs);
        assert_eq!(*abs, vfs);
    }
}
