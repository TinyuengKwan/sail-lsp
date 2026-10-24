//! Documentation extraction.
//! RA's `Documentation<'db>` wraps `Cow<'db, str>` with a lifetime
//! parameter. Sail simplifies to owned `String` since we don't have
//! salsa-borrowed doc strings (docs come from ItemTree cloning).

/// Wrapper around a documentation string.
///
/// ```text
/// pub struct Documentation<'db>(Cow<'db, str>);
/// ```
///
/// Sail simplification: always owned (no `Cow`), since doc strings
/// are cloned from ItemTree entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Documentation(String);

impl Documentation {
    /// Create from an owned string.
    pub fn new(s: String) -> Self {
        Self(s)
    }

    /// View as string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for Documentation {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Documentation {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl std::fmt::Display for Documentation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for Documentation {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// Trait for HIR types that have documentation.
///
/// ```text
/// pub trait HasDocs: HasAttrs + Copy {
///     fn docs(self, db: &dyn HirDatabase) -> Option<Documentation<'_>>;
/// }
/// ```
///
/// Sail simplification: no `HasAttrs` bound (Sail has no attributes),
/// takes `&self` instead of `self` (no Copy requirement).
pub trait HasDocs {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation>;
}

/// Helper: look up doc comment from ItemTree for a DefLocation.
fn docs_from_loc(db: &dyn salsa::Database, loc: &hir::DefLocation) -> Option<Documentation> {
    loc.doc_text(db).map(Documentation::new)
}

// Implements HasDocs for each hir type that wraps a DefLocation.
impl HasDocs for hir::Function {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation> {
        docs_from_loc(db, &hir::Def::Function(*self).location())
    }
}

impl HasDocs for hir::TypeDef {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation> {
        docs_from_loc(db, &hir::Def::TypeDef(*self).location())
    }
}

impl HasDocs for hir::Register {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation> {
        docs_from_loc(db, &hir::Def::Register(*self).location())
    }
}

impl HasDocs for hir::Mapping {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation> {
        docs_from_loc(db, &hir::Def::Mapping(*self).location())
    }
}

impl HasDocs for hir::Def {
    fn docs(&self, db: &dyn salsa::Database) -> Option<Documentation> {
        match self {
            hir::Def::Function(f) => f.docs(db),
            hir::Def::TypeDef(t) => t.docs(db),
            hir::Def::Register(r) => r.docs(db),
            hir::Def::Mapping(m) => m.docs(db),
            other => docs_from_loc(db, &other.location()),
        }
    }
}
