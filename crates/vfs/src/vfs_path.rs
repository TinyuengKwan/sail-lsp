//! Abstract-ish representation of paths for VFS.
//!
//! ALIGN(ra): Supports both real filesystem paths (`AbsPathBuf`) and
//! virtual in-memory paths (`VirtualPath`). Virtual paths are platform-
//! independent and primarily used in tests to avoid Windows/Linux differences.

use std::fmt;

use paths::{AbsPath, AbsPathBuf, RelPath};

/// Path in [`Vfs`].
///
/// Long-term, we want to support files which do not reside in the file-system,
/// so we treat `VfsPath`s as opaque identifiers.
///
/// [`Vfs`]: crate::Vfs
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct VfsPath(VfsPathRepr);

impl VfsPath {
    /// Creates an "in-memory" path from `/`-separated string.
    ///
    /// This is most useful for testing, to avoid windows/linux differences.
    ///
    /// # Panics
    ///
    /// Panics if `path` does not start with `'/'`.
    pub fn new_virtual_path(path: String) -> VfsPath {
        assert!(path.starts_with('/'));
        VfsPath(VfsPathRepr::VirtualPath(VirtualPath(path)))
    }

    /// Create a VfsPath from an `AbsPathBuf`.
    pub fn new(path: AbsPathBuf) -> Self {
        VfsPath(VfsPathRepr::PathBuf(path))
    }

    /// Returns the `AbsPath` representation of `self` if `self` is on the file system.
    pub fn as_path(&self) -> Option<&AbsPath> {
        match &self.0 {
            VfsPathRepr::PathBuf(it) => Some(it.as_path()),
            VfsPathRepr::VirtualPath(_) => None,
        }
    }

    pub fn into_abs_path(self) -> Option<AbsPathBuf> {
        match self.0 {
            VfsPathRepr::PathBuf(it) => Some(it),
            VfsPathRepr::VirtualPath(_) => None,
        }
    }

    /// Creates a new `VfsPath` with `path` adjoined to `self`.
    pub fn join(&self, path: &str) -> Option<VfsPath> {
        match &self.0 {
            VfsPathRepr::PathBuf(it) => {
                let res = it.join(path).normalize();
                Some(VfsPath(VfsPathRepr::PathBuf(res)))
            }
            VfsPathRepr::VirtualPath(it) => {
                let res = it.join(path)?;
                Some(VfsPath(VfsPathRepr::VirtualPath(res)))
            }
        }
    }

    /// Remove the last component of `self` if there is one.
    ///
    /// If `self` has no component, returns `false`; else returns `true`.
    pub fn pop(&mut self) -> bool {
        match &mut self.0 {
            VfsPathRepr::PathBuf(it) => it.pop(),
            VfsPathRepr::VirtualPath(it) => it.pop(),
        }
    }

    /// Check if this path starts with the given prefix.
    pub fn starts_with(&self, other: &VfsPath) -> bool {
        match (&self.0, &other.0) {
            (VfsPathRepr::PathBuf(lhs), VfsPathRepr::PathBuf(rhs)) => lhs.starts_with(rhs),
            (VfsPathRepr::VirtualPath(lhs), VfsPathRepr::VirtualPath(rhs)) => lhs.starts_with(rhs),
            (VfsPathRepr::PathBuf(_) | VfsPathRepr::VirtualPath(_), _) => false,
        }
    }

    pub fn strip_prefix(&self, other: &VfsPath) -> Option<&RelPath> {
        match (&self.0, &other.0) {
            (VfsPathRepr::PathBuf(lhs), VfsPathRepr::PathBuf(rhs)) => lhs.strip_prefix(rhs),
            (VfsPathRepr::VirtualPath(lhs), VfsPathRepr::VirtualPath(rhs)) => lhs.strip_prefix(rhs),
            (VfsPathRepr::PathBuf(_) | VfsPathRepr::VirtualPath(_), _) => None,
        }
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
        match &self.0 {
            VfsPathRepr::PathBuf(p) => p.name_and_extension(),
            VfsPathRepr::VirtualPath(p) => p.name_and_extension(),
        }
    }

    /// Encode the path into the given buffer (for prefix matching).
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        let tag = match &self.0 {
            VfsPathRepr::PathBuf(_) => 0,
            VfsPathRepr::VirtualPath(_) => 1,
        };
        buf.push(tag);
        match &self.0 {
            VfsPathRepr::PathBuf(path) => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    buf.extend(path.as_os_str().as_bytes());
                }
                #[cfg(not(unix))]
                {
                    buf.extend(path.as_os_str().to_string_lossy().as_bytes());
                }
            }
            VfsPathRepr::VirtualPath(VirtualPath(s)) => buf.extend(s.as_bytes()),
        }
    }
}

/// Internal, private representation of [`VfsPath`].
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
enum VfsPathRepr {
    PathBuf(AbsPathBuf),
    VirtualPath(VirtualPath),
}

impl From<AbsPathBuf> for VfsPath {
    fn from(path: AbsPathBuf) -> Self {
        VfsPath(VfsPathRepr::PathBuf(path.normalize()))
    }
}

impl fmt::Display for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            VfsPathRepr::PathBuf(it) => it.fmt(f),
            VfsPathRepr::VirtualPath(VirtualPath(it)) => it.fmt(f),
        }
    }
}

impl fmt::Debug for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Debug for VfsPathRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            VfsPathRepr::PathBuf(it) => it.fmt(f),
            VfsPathRepr::VirtualPath(VirtualPath(it)) => it.fmt(f),
        }
    }
}

impl PartialEq<AbsPath> for VfsPath {
    fn eq(&self, other: &AbsPath) -> bool {
        match &self.0 {
            VfsPathRepr::PathBuf(lhs) => lhs == other,
            VfsPathRepr::VirtualPath(_) => false,
        }
    }
}

impl PartialEq<VfsPath> for AbsPath {
    fn eq(&self, other: &VfsPath) -> bool {
        other == self
    }
}

/// `/`-separated virtual path.
///
/// This is used to describe files that do not reside on the file system.
#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
struct VirtualPath(String);

impl VirtualPath {
    fn starts_with(&self, other: &VirtualPath) -> bool {
        self.0.starts_with(&other.0)
    }

    fn strip_prefix(&self, base: &VirtualPath) -> Option<&RelPath> {
        <_ as AsRef<paths::Utf8Path>>::as_ref(&self.0)
            .strip_prefix(&base.0)
            .ok()
            .map(RelPath::new_unchecked)
    }

    fn pop(&mut self) -> bool {
        let pos = match self.0.rfind('/') {
            Some(pos) => pos,
            None => return false,
        };
        self.0 = self.0[..pos].to_string();
        true
    }

    fn join(&self, mut path: &str) -> Option<VirtualPath> {
        let mut res = self.clone();
        while path.starts_with("../") {
            if !res.pop() {
                return None;
            }
            path = &path["../".len()..];
        }
        path = path.trim_start_matches("./");
        res.0 = format!("{}/{path}", res.0);
        Some(res)
    }

    fn name_and_extension(&self) -> Option<(&str, Option<&str>)> {
        let file_path = if self.0.ends_with('/') { &self.0[..&self.0.len() - 1] } else { &self.0 };
        let file_name = match file_path.rfind('/') {
            Some(position) => &file_path[position + 1..],
            None => file_path,
        };

        if file_name.is_empty() {
            None
        } else {
            let mut file_stem_and_extension = file_name.rsplitn(2, '.');
            let extension = file_stem_and_extension.next();
            let file_stem = file_stem_and_extension.next();

            match (file_stem, extension) {
                (None, None) => None,
                (None | Some(""), Some(_)) => Some((file_name, None)),
                (Some(file_stem), extension) => Some((file_stem, extension)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_path_basic() {
        let path = VfsPath::new_virtual_path("/foo/bar.sail".to_string());
        assert!(path.as_path().is_none());
        assert_eq!(path.name_and_extension(), Some(("bar", Some("sail"))));
    }

    #[test]
    fn virtual_path_join() {
        let base = VfsPath::new_virtual_path("/project".to_string());
        let child = base.join("src/main.sail").unwrap();
        assert_eq!(format!("{child}"), "/project/src/main.sail");
    }

    #[test]
    fn virtual_path_pop() {
        let mut path = VfsPath::new_virtual_path("/foo/bar".to_string());
        assert!(path.pop());
        assert_eq!(format!("{path}"), "/foo");
        assert!(path.pop());
        assert_eq!(format!("{path}"), "");
        assert!(!path.pop());
    }

    #[test]
    fn virtual_path_strip_prefix() {
        let base = VfsPath::new_virtual_path("/project".to_string());
        let child = VfsPath::new_virtual_path("/project/src/main.sail".to_string());
        let rel = child.strip_prefix(&base);
        assert!(rel.is_some());
        assert_eq!(rel.unwrap().as_str(), "src/main.sail");
    }

    #[test]
    fn virtual_path_starts_with() {
        let base = VfsPath::new_virtual_path("/project".to_string());
        let child = VfsPath::new_virtual_path("/project/src/main.sail".to_string());
        assert!(child.starts_with(&base));

        let other = VfsPath::new_virtual_path("/other".to_string());
        assert!(!child.starts_with(&other));
    }

    #[test]
    fn virtual_path_encode() {
        let path = VfsPath::new_virtual_path("/tmp/test.sail".to_string());
        let mut buf = Vec::new();
        path.encode(&mut buf);
        assert_eq!(buf[0], 1); // tag: virtual
        assert_eq!(&buf[1..], b"/tmp/test.sail");
    }

    #[test]
    #[cfg(unix)]
    fn real_path_encode() {
        let path = VfsPath::new(AbsPathBuf::assert_utf8("/tmp/test.sail".into()));
        let mut buf = Vec::new();
        path.encode(&mut buf);
        assert_eq!(buf[0], 0); // tag: real path
        assert!(buf.len() > 1);
    }

    #[test]
    #[cfg(unix)]
    fn cross_type_no_match() {
        let real = VfsPath::new(AbsPathBuf::assert_utf8("/tmp/test.sail".into()));
        let virt = VfsPath::new_virtual_path("/tmp/test.sail".to_string());
        // Different repr => not equal
        assert_ne!(real, virt);
    }
}
