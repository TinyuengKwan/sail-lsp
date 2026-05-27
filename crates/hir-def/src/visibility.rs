//! HIR-level visibility: `Public` (default) or `@private` (file-scoped).

use base_db::FileId;

/// Unresolved visibility as it appears in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawVisibility {
    /// The definition is visible everywhere (Sail default).
    #[default]
    Public,
    /// The definition is restricted to the declaring file
    /// (from `@private` attribute).
    Private,
}

/// Resolved visibility of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// Unrestricted — visible everywhere.
    Public,
    /// Restricted to the declaring file.
    FilePrivate(FileId),
}

impl Visibility {
    /// Resolve a `RawVisibility` into a `Visibility`.
    pub fn resolve(raw: RawVisibility, declaring_file: FileId) -> Self {
        match raw {
            RawVisibility::Public => Visibility::Public,
            RawVisibility::Private => Visibility::FilePrivate(declaring_file),
        }
    }

    /// Whether this item is visible from the given file.
    pub fn is_visible_from(self, from_file: FileId) -> bool {
        match self {
            Visibility::Public => true,
            Visibility::FilePrivate(declaring_file) => from_file == declaring_file,
        }
    }

    /// Whether this item is visible from other files.
    pub fn is_visible_from_other_files(self) -> bool {
        matches!(self, Visibility::Public)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_is_visible_everywhere() {
        let vis = Visibility::Public;
        assert!(vis.is_visible_from(FileId::from_raw(0)));
        assert!(vis.is_visible_from(FileId::from_raw(1)));
        assert!(vis.is_visible_from(FileId::from_raw(999)));
        assert!(vis.is_visible_from_other_files());
    }

    #[test]
    fn private_is_only_visible_in_declaring_file() {
        let vis = Visibility::FilePrivate(FileId::from_raw(42));
        assert!(vis.is_visible_from(FileId::from_raw(42)));
        assert!(!vis.is_visible_from(FileId::from_raw(0)));
        assert!(!vis.is_visible_from(FileId::from_raw(43)));
        assert!(!vis.is_visible_from_other_files());
    }

    #[test]
    fn resolve_public() {
        let vis = Visibility::resolve(RawVisibility::Public, FileId::from_raw(0));
        assert_eq!(vis, Visibility::Public);
    }

    #[test]
    fn resolve_private() {
        let vis = Visibility::resolve(RawVisibility::Private, FileId::from_raw(5));
        assert_eq!(vis, Visibility::FilePrivate(FileId::from_raw(5)));
    }
}
