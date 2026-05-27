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
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation>;
}

/// Helper: look up doc comment from ItemTree for a DefLocation.
fn docs_from_loc(
    db: &dyn hir_def::db::DefDatabase,
    loc: &hir::DefLocation,
) -> Option<Documentation> {
    let item_tree = hir_def::def_query::file_item_tree(db, loc.file_text).as_ref()?;
    let def_map = hir_def::def_query::crate_def_map(db, loc.file_text).as_ref()?;
    let raw_id = loc.raw_def_id();
    let def_data = def_map.0.get(raw_id)?;
    let mod_item = item_tree.top_level_items().get(def_data.item_tree_index)?;
    mod_item.doc(item_tree).map(|s| Documentation::new(s.to_string()))
}

/// Implements HasDocs for each hir type that wraps a DefLocation.

impl HasDocs for hir::Function {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        docs_from_loc(db, &hir::ModuleDef::Function(*self).location())
    }
}

impl HasDocs for hir::Adt {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        docs_from_loc(db, &hir::ModuleDef::TypeDef(*self).location())
    }
}

impl HasDocs for hir::AdtId {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        self.type_def().docs(db)
    }
}

impl HasDocs for hir::Register {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        docs_from_loc(db, &hir::ModuleDef::Register(*self).location())
    }
}

impl HasDocs for hir::Mapping {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        docs_from_loc(db, &hir::ModuleDef::Mapping(*self).location())
    }
}

impl HasDocs for hir::ModuleDef {
    fn docs(&self, db: &dyn hir_def::db::DefDatabase) -> Option<Documentation> {
        match self {
            hir::ModuleDef::Function(f) => f.docs(db),
            hir::ModuleDef::TypeDef(t) => t.docs(db),
            hir::ModuleDef::Register(r) => r.docs(db),
            hir::ModuleDef::Mapping(m) => m.docs(db),
            hir::ModuleDef::Let(loc) | hir::ModuleDef::Overload(loc) => docs_from_loc(db, loc),
        }
    }
}
