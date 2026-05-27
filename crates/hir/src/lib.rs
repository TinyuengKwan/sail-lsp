//! High-level Sail HIR — the `Semantics` facade and public HIR types.
//!
//! # Architecture (RA alignment)
//!
//! The `hir` crate does NOT depend on `ide-db` or `ide`.
//! Dependency direction: `ide-db` → `hir` → `hir-ty` → `hir-def` → `base-db`.
//!
//! The IDE layer should depend on `hir` rather than reaching into
//! `hir-def` directly: this crate owns the cross-cutting analysis
//! that aggregates lower layers into a developer-facing API.

pub mod db;
pub mod diagnostics;
pub mod display;
mod from_id;
pub mod has_source;
// RA exposes call_hierarchy in the ide crate, not hir. Sail needs it in
// hir so that ide-db can build the WorkspaceCallGraph without importing ide.
pub mod hir_query;
pub mod semantics;
pub(crate) mod source_analyzer;
pub mod symbols;

/// Re-export the callgraph module from hir-def so existing
/// `hir::callgraph::*` paths continue to resolve.
pub mod callgraph {
    pub use hir_def::callgraph::*;
}

pub use callgraph::{cached_workspace_callgraph, CallGraph, CallSite, WorkspaceCallGraph};

// Re-export database traits
pub use db::{DefDatabase, HirDatabase};

// HasSource trait
pub use has_source::HasSource;

// Re-export Semantics
pub use semantics::Semantics;

// Each type wraps an internal ID.  Data is accessed via methods that
// look up the DefMap.  This matches RA where `Function { id }` and
// `function.name(db)` queries the database.
//
// For Sail, the ID is `(FileId, DefId)` since DefId is per-file.

use hir_def::item_tree::ItemKind;
use hir_def::name::Name;
use hir_def::nameres::DefId;
use hir_def::Span;

/// Location of a definition: which file and which typed ID within that file.
///
/// This is the Sail equivalent of RA's various `*Id` types (FunctionId,
/// StructId, etc.). Each `def_id` carries the item kind through the
/// `ModuleDefId` enum (7 typed variants).
///
/// Stores `FileText` (salsa input) instead of `FileId` so that
/// hir public type methods can query the database directly:
///   `fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name`
/// This.
// newtypes (FunctionId, StructId …) interned in a global arena. Sail avoids
// the interning infrastructure by embedding the kind in ModuleDefId.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefLocation {
    pub file_text: base_db::FileText,
    pub def_id: hir_def::ModuleDefId,
}

impl std::fmt::Debug for DefLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefLocation").field("def_id", &self.def_id).finish()
    }
}

impl DefLocation {
    /// Get the raw DefId for DefMap lookups.
    pub fn raw_def_id(&self) -> DefId {
        DefId(self.def_id.as_raw())
    }

    /// Get the FileId from the stored FileText.
    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.file_text.file_id(db)
    }

    /// Look up the name of this definition via the database.
    pub fn lookup_name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        hir_def::def_query::crate_def_map(db, self.file_text)
            .as_ref()
            .and_then(|dm| dm.0.get(self.raw_def_id()))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| Name::new("<unknown>"))
    }

    /// Look up the signature text of this definition from the ItemTree.
    ///
    /// Returns the signature string (e.g. `"int -> bool"`) or `None` if
    /// the item tree is unavailable or the index is out of range.
    pub fn lookup_signature(&self, db: &dyn hir_def::db::DefDatabase) -> Option<String> {
        let def_map = db.def_map(self.file_text)?;
        let def_data = def_map.get(self.raw_def_id())?;
        let item_tree = db.file_item_tree(self.file_text)?;
        let items = item_tree.top_level_items();
        let mod_item = items.get(def_data.item_tree_index)?;
        let sig = mod_item.signature(item_tree);
        if sig.is_empty() {
            None
        } else {
            Some(sig.to_string())
        }
    }
}

/// A module (file) in the workspace.
/// In Sail, each file is exactly one module — there are no nested modules.
/// This type wraps `FileId` and provides the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Module {
    pub id: base_db::FileId,
}

/// Backward-compat alias: `File` → `Module`.
#[deprecated(note = "use Module instead (RA naming)")]
pub type File = Module;

/// A callable definition (function or mapping clause).
/// Data is accessed via methods that look up the DefMap.
// Difference: id is DefLocation (file+kind) not an interned FunctionId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Function {
    pub(crate) id: DefLocation,
}

impl Function {
    /// Takes `db` instead of `&DefMap``).
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A type definition (struct, union, enum, bitfield, newtype, type alias).
// Sail collapses them into a single Adt with a TypeDefKind discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Adt {
    pub(crate) id: DefLocation,
    pub(crate) kind: TypeDefKind,
}

/// Backward-compat alias: `TypeDef` → `Adt`.
pub type TypeDef = Adt;

/// Sub-kinds for TypeDef.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeDefKind {
    Struct,
    Union,
    Enum,
    Bitfield,
    Newtype,
    TypeAlias,
}

impl Adt {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn kind(&self) -> TypeDefKind {
        self.kind
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

impl TypeDefKind {
    pub fn from_item_kind(kind: ItemKind) -> Option<Self> {
        match kind {
            ItemKind::Struct => Some(Self::Struct),
            ItemKind::Union => Some(Self::Union),
            ItemKind::Enum => Some(Self::Enum),
            ItemKind::Bitfield => Some(Self::Bitfield),
            ItemKind::Newtype => Some(Self::Newtype),
            ItemKind::TypeAlias => Some(Self::TypeAlias),
            _ => None,
        }
    }
}

//
// Independent types, each holding its own `DefLocation`.
// `Enum { id: EnumId }` are completely separate types.

/// A struct definition.
///
/// `pub struct Struct { pub(crate) id: StructId }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Struct {
    pub(crate) id: DefLocation,
}

impl Struct {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A union definition.
///
/// `pub struct Union { pub(crate) id: UnionId }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Union {
    pub(crate) id: DefLocation,
}

impl Union {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// An enum definition.
///
/// `pub struct Enum { pub(crate) id: EnumId }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Enum {
    pub(crate) id: DefLocation,
}

impl Enum {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A data type — struct, union, or enum (narrower than `Adt`).
///
/// ```text
/// pub enum AdtId {
///     Struct(Struct),
///     Union(Union),
///     Enum(Enum),
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdtId {
    Struct(Struct),
    Union(Union),
    Enum(Enum),
}

impl AdtId {
    /// Construct from an Adt if it's an ADT kind.
    pub fn from_adt(td: Adt) -> Option<Self> {
        match td.kind {
            TypeDefKind::Struct => Some(AdtId::Struct(Struct { id: td.id })),
            TypeDefKind::Union => Some(AdtId::Union(Union { id: td.id })),
            TypeDefKind::Enum => Some(AdtId::Enum(Enum { id: td.id })),
            _ => None,
        }
    }

    /// Backward-compat alias for `from_adt`.
    pub fn from_type_def(td: Adt) -> Option<Self> {
        Self::from_adt(td)
    }

    /// Get the inner Adt.
    pub fn type_def(&self) -> Adt {
        match self {
            AdtId::Struct(s) => Adt { id: s.id, kind: TypeDefKind::Struct },
            AdtId::Union(u) => Adt { id: u.id, kind: TypeDefKind::Union },
            AdtId::Enum(e) => Adt { id: e.id, kind: TypeDefKind::Enum },
        }
    }

    /// Get the name of this ADT.
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.type_def().name(db)
    }

    /// Get the file containing this ADT.
    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.type_def().file_id(db)
    }

    /// Downcast to struct.
    pub fn as_struct(&self) -> Option<Struct> {
        match self {
            AdtId::Struct(s) => Some(*s),
            _ => None,
        }
    }

    /// Downcast to union.
    pub fn as_union(&self) -> Option<Union> {
        match self {
            AdtId::Union(u) => Some(*u),
            _ => None,
        }
    }

    /// Downcast to enum.
    pub fn as_enum(&self) -> Option<Enum> {
        match self {
            AdtId::Enum(e) => Some(*e),
            _ => None,
        }
    }
}

impl From<Adt> for Option<AdtId> {
    fn from(td: Adt) -> Self {
        AdtId::from_adt(td)
    }
}

/// A register definition (Sail-specific).
// hardware-register name to a type; treated as a distinct DefKind in DefMap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Register {
    pub(crate) id: DefLocation,
}

impl Register {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A mapping definition (Sail-specific — bidirectional encoding/decoding).
// encode/decode functions; appears as a distinct kind in DefMap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mapping {
    pub(crate) id: DefLocation,
}

impl Mapping {
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// Any top-level module-level definition.
/// Note: RA also has `Definition` in ide-db which wraps ModuleDef +
/// Local + GenericParam etc.  This is the compiler-level enum.
// Static/Trait/TraitAlias/TypeAlias/BuiltinType/Macro). Sail has 6.
// Adt collapses RA's Adt+TypeAlias+BuiltinType into one umbrella.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModuleDef {
    Function(Function),
    TypeDef(Adt),
    Register(Register),
    Mapping(Mapping),
    /// A let/var binding at top level.
    Let(DefLocation),
    /// An overload declaration.
    Overload(DefLocation),
}

impl ModuleDef {
    /// Get the name of this definition.
    ///
    /// Takes `db``).
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Name {
        match self {
            Self::Function(f) => f.name(db),
            Self::TypeDef(t) => t.name(db),
            Self::Register(r) => r.name(db),
            Self::Mapping(m) => m.name(db),
            Self::Let(loc) | Self::Overload(loc) => loc.lookup_name(db),
        }
    }

    /// Get the file containing this definition.
    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> base_db::FileId {
        match self {
            Self::Function(f) => f.file_id(db),
            Self::TypeDef(t) => t.file_id(db),
            Self::Register(r) => r.file_id(db),
            Self::Mapping(m) => m.file_id(db),
            Self::Let(loc) | Self::Overload(loc) => loc.file_id(db),
        }
    }

    /// Get the DefLocation.
    pub fn location(&self) -> DefLocation {
        match self {
            Self::Function(f) => f.id,
            Self::TypeDef(t) => t.id,
            Self::Register(r) => r.id,
            Self::Mapping(m) => m.id,
            Self::Let(loc) | Self::Overload(loc) => *loc,
        }
    }
}

/// Resolution of a path/name to a definition.
// BuiltinAttr/ConstParam/DeriveHelper). Sail has 3.
// Missing: SelfType (no impl blocks), ConstParam (no const generics),
// BuiltinAttr/DeriveHelper (no derive macro machinery).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathResolution {
    /// Resolved to a top-level definition.
    Def(ModuleDef),
    /// Resolved to a local variable/binding.
    Local(Local),
    /// Resolved to a type parameter/variable ('n, 'm, etc.).
    ///
    /// Sail calls these "type variables" but RA calls them "generic params".
    TypeParam(GenericParam),
}

// Backward-compat: PathResolution::TypeVar is an alias for TypeParam.
impl PathResolution {
    /// Backward-compat alias. Prefer `TypeParam` for new code.
    #[allow(non_snake_case)]
    pub fn TypeVar(tv: GenericParam) -> Self {
        PathResolution::TypeParam(tv)
    }

    /// Construct from a `TypeNs` resolution.
    pub fn from_type_ns(ns: hir_def::resolver::TypeNs) -> Self {
        match ns {
            hir_def::resolver::TypeNs::BuiltinType(name) => {
                // Built-in types resolve as TypeParam for now.
                // RA resolves them to `Definition::BuiltinType` at the
                // ide-db level; hir level wraps as TypeVar.
                PathResolution::TypeParam(GenericParam { name })
            }
            hir_def::resolver::TypeNs::Workspace(name) => {
                PathResolution::TypeParam(GenericParam { name })
            }
            hir_def::resolver::TypeNs::AdtId(_) | hir_def::resolver::TypeNs::TypeAliasId(_) => {
                PathResolution::TypeParam(TypeVar { name: Name::new("") })
            }
        }
    }

    /// Construct from a `ValueNs` resolution.
    pub fn from_value_ns(ns: hir_def::resolver::ValueNs) -> Self {
        match ns {
            hir_def::resolver::ValueNs::LocalBinding(_id) => {
                PathResolution::Local(Local { name: Name::new(""), span: Span::new(0, 0) })
            }
            hir_def::resolver::ValueNs::EnumVariantId(name) => {
                // Enum variant constructor
                PathResolution::Local(Local { name, span: Span::new(0, 0) })
            }
            hir_def::resolver::ValueNs::Workspace(name) => {
                PathResolution::Local(Local { name, span: Span::new(0, 0) })
            }
            _ => PathResolution::Local(Local { name: Name::new(""), span: Span::new(0, 0) }),
        }
    }
}

/// A local variable binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Local {
    pub name: Name,
    pub span: Span,
}

/// A type parameter / type variable ('n, 'm, etc.).
///
/// RA calls them "generic params" — we use the RA name for alignment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParam {
    pub name: Name,
}

/// Backward-compat alias: `TypeVar` → `GenericParam`.
pub type TypeVar = GenericParam;

/// Backward-compat alias: `TypeParam` → `GenericParam`.
pub type TypeParam = GenericParam;

/// Classifies what kind of name reference is at a position.
#[derive(Debug, Clone)]
pub enum NameRefKind {
    /// A path reference (variable, function, type name)
    Path(PathResolution),
    /// A field access (expr.field)
    FieldAccess { field_name: String },
    /// A function/method call
    Call { callee_name: String, resolution: Option<PathResolution> },
    /// Unresolved
    Unresolved,
}
