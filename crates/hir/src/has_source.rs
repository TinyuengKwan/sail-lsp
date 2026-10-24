//! Provides set of implementations for hir's objects that allows
//! getting back the location in file.
//! Each public HIR type implements `HasSource` to retrieve its
//! source syntax node pointer + file. The implementation delegates
//! to `hir_def::src::def_source` which looks up the item's span
//! in the ItemTree.

use hir_def::in_file::InFile;
use syntax::SyntaxNodePtr;

use crate::{Def, DefLocation, Function, Mapping, Register, TypeDef};

/// Trait for HIR objects that have a source location in a file.
///
/// ```ignore
/// pub trait HasSource: Sized {
///     type Ast: AstNode;
///     fn source(self, db: &dyn HirDatabase) -> Option<InFile<Self::Ast>>;
/// }
/// ```
///
/// # Sail simplification
///
/// RA's `HasSource` takes `&dyn HirDatabase` and returns typed
/// `InFile<ast::Fn>` etc. Sail's version takes the pre-fetched
/// `DefMap` + `ItemTree` (which the caller already has from salsa)
/// and returns `InFile<SyntaxNodePtr>`. This avoids adding a
/// database dependency to the trait while providing the same
/// file + source-location pairing.
///
/// Once salsa integration deepens (e.g., a `DefDatabase` trait
/// with `item_tree(file_id)` and `def_map(file_id)` queries), this
/// trait can be upgraded to take `&dyn salsa::Database` like RA.
/// Takes `db` instead of `&DefMap + &ItemTree`.
pub trait HasSource: Sized {
    /// The AST type returned by `source()`.
    ///
    /// `SyntaxNodePtr` (untyped pointer); once parsing is integrated
    /// into the source lookup path, impls will specialize to typed
    /// AST nodes (e.g., `ast::CallableDef` for Function).
    type Ast;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<Self::Ast>>;
}

fn loc_source(
    db: &dyn salsa::Database,
    loc: &DefLocation,
    def_map: &hir_def::nameres::DefMap,
    item_tree: &hir_def::item_tree::ItemTree,
) -> Option<InFile<SyntaxNodePtr>> {
    hir_def::def_id_source(loc.file_id(db), loc.raw_def_idx(), def_map, item_tree)
}

/// Helper: query DefMap + ItemTree from db for a DefLocation.
fn loc_source_from_db(
    db: &dyn salsa::Database,
    loc: &DefLocation,
) -> Option<InFile<SyntaxNodePtr>> {
    let dm = hir_def::def_query::file_def_map(db, loc.file_text).as_ref()?;
    let it = hir_def::def_query::file_item_tree(db, loc.file_text).as_ref()?;
    loc_source(db, loc, &dm.0, it)
}

impl HasSource for Function {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        loc_source_from_db(db, &self.id)
    }
}

impl HasSource for TypeDef {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        loc_source_from_db(db, &self.id)
    }
}

impl HasSource for Register {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        loc_source_from_db(db, &self.id)
    }
}

impl HasSource for Mapping {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        loc_source_from_db(db, &self.id)
    }
}

impl HasSource for Def {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        match self {
            Def::Function(f) => f.source(db),
            Def::TypeDef(t) => t.source(db),
            Def::Register(r) => r.source(db),
            Def::Mapping(m) => m.source(db),
            other => loc_source_from_db(db, &other.location()),
        }
    }
}

impl HasSource for DefLocation {
    type Ast = SyntaxNodePtr;

    fn source(&self, db: &dyn salsa::Database) -> Option<InFile<SyntaxNodePtr>> {
        loc_source_from_db(db, self)
    }
}
