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

pub use hir_def::source_order;
pub use hir_def::visibility;

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

pub use hir_ty::solver_budget::{SolverBudget, SolverLimits};
/// Cooperative cancellation for a long-running query.
pub use hir_ty::CancellationToken;

// HasSource trait
pub use has_source::HasSource;

// Re-export Semantics
pub use semantics::{Semantics, TopLevelNames};

/// Types from the layers below that `ide` needs to name.
///
/// rust-analyzer's `ide` depends on `hir` and `syntax` and nothing else, so
/// anything a feature has to name comes through here. Re-exporting is what
/// keeps that true without forcing every type to be redefined.
pub use hir_ty::display::HirDisplay;
pub use hir_ty::infer::{InferenceResult, SubrangeProblem, TypeCheckResult};

/// Type inference, driven per callable. Sail's numeric and bitvector
/// widths make these queries the expensive ones, so they are named here
/// rather than reached for a layer down.
pub mod ty_query {
    pub use hir_ty::query::*;
}

/// Sail's types: scalars, bitvectors of a width, and the numeric
/// expressions a width or a `range` bound can be written as.
pub mod ty {
    pub use hir_ty::infer::Ty;
    pub use hir_ty::ty::{Scalar, TyArg, TyKind};
}

/// The inference engine's own vocabulary: the numeric constraints Sail's
/// dependent types generate, and what the solver made of them.
pub mod infer {
    pub use hir_ty::infer::*;
}
pub use hir_ty::infer::InferenceDiagnostic;

/// The Z3 backend that discharges those constraints.
///
/// Optional, and re-exported under the same `cfg` as the module it names —
/// `hir-ty` has gated it since the day it was written, and this door did
/// not, so `--no-default-features` failed to compile here rather than
/// dropping the solver. The distributed server pays for that: a binary
/// built against one z3 will not start beside another
/// (`libz3.so.4.16 => not found`).
#[cfg(feature = "z3-solver")]
pub mod z3_solver {
    pub use hir_ty::infer::z3_solver::*;
}
pub use ty::{Scalar, Ty, TyKind};

/// Whether a type definition can be laid out at all, or recurses forever.
pub mod representability {
    pub use hir_ty::representability::*;
}

pub use hir_def::item_tree::generated;
/// What kind of definition something is, in Sail's own vocabulary.
///
/// The names are the ones the language uses — `Mapping`, `Bitfield`,
/// `Register`, `ScatteredClause`, `Overload`, `Outcome` — not a translation
/// into another language's categories. rust-analyzer's `ide` names
/// `hir::Def` and never `hir_def`; this is the same door, with Sail's
/// words on it.
pub use hir_def::item_tree::DefKind;
/// A definition's shape as the item tree records it.
pub use hir_def::item_tree::{
    extract_field_decls, DefItem, FieldDecl, ItemTree, TypeDefKind as TypeDefShape,
};

/// Sail's effects: `rreg`, `wreg`, `throw`, `exit`, `undefined` and the rest.
pub use hir_def::bodies::{CallableBodies, CallableBody, EffectTag};

/// Bitfield layout — a Sail construct with no rust-analyzer counterpart.
pub mod bitfield {
    pub use hir_def::bitfield::*;
}

/// `$include` resolution. Sail's file graph, where RA has macro expansion.
pub mod include_graph {
    pub use hir_def::include_graph::*;
}

/// Sail's own diagnostic vocabulary: scattered-definition completeness,
/// effect mismatches, and the type errors upstream reports.
pub mod scattered {
    pub use hir_def::scattered::*;
}
pub mod type_error {
    pub use hir_def::type_error::*;
}
pub mod message {
    pub use hir_def::message::*;
}
pub mod effects {
    pub use hir_def::effects::*;
}

/// The scope tree a body lowers to: which bindings are visible where, so
/// two `let x` in one function are different names.
pub mod scope {
    pub use hir_def::expr_store::scope::{ExprScopes, ScopeEntry, ScopeId, ScopeOrigin};
}
pub use scope::ExprScopes;

pub use hir_def::analysis_scope::AnalysisScope;
pub use hir_def::{
    builtin_type_kinds, is_builtin_type_level_fn, is_builtin_type_name, GlobalDef, Kind,
    WorkspaceDefMap,
};
pub use hir_expand::preprocess;
pub use hir_expand::query::include_paths;

/// A position or range, paired with the file it is in.
pub use hir_expand::in_file::{FilePosition, FileRange, InFile};

/// The expression and pattern arenas a body lowers to.
pub use hir_def::{Body, BodySourceMap, Expr, ExprId, Pat, PatId, Statement};

/// Salsa queries the IDE layer drives directly.
pub mod def_query {
    pub use hir_def::def_query::*;
}

/// A callable's signature as the item tree recorded it.
pub mod callable_info {
    pub use hir_def::callable_info::*;
}

/// Name resolution within one file.
pub use hir_def::nameres::DefMap;

// Each type wraps an internal ID.  Data is accessed via methods that
// look up the DefMap.  This matches RA where `Function { id }` and
// `function.name(db)` queries the database.
//
// For Sail, the ID is `(FileId, DefIdx)` since DefIdx is per-file.

/// An interned identifier. Sail's `id`, plus the `'n` of a `kid`.
pub use hir_def::name::Name;
use hir_def::nameres::DefIdx;
/// A byte range in a source file.
pub use hir_def::Span;

/// Location of a definition: which file and which typed ID within that file.
///
/// This is the Sail equivalent of RA's various `*Id` types (FunctionId,
/// StructId, etc.). Each `def_id` carries the item kind through the
/// `DefId` enum (7 typed variants).
///
/// Stores `FileText` (salsa input) instead of `FileId` so that
/// hir public type methods can query the database directly:
///   `fn name(&self, db: &dyn salsa::Database) -> Name`
/// This.
// newtypes (FunctionId, StructId …) interned in a global arena. Sail avoids
// the interning infrastructure by embedding the kind in DefId.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefLocation {
    pub file_text: base_db::FileText,
    pub def_id: hir_def::DefId,
}

impl std::fmt::Debug for DefLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefLocation").field("def_id", &self.def_id).finish()
    }
}

impl DefLocation {
    /// The item-tree entry this location points at.
    ///
    /// Four places did this by hand — signature text, doc comment, rename,
    /// hover — each fetching the item tree and the def map and indexing one
    /// into the other. It is one lookup, and it belongs with the location.
    pub fn item<'a>(&self, db: &'a dyn salsa::Database) -> Option<(&'a ItemTree, DefItem)> {
        let item_tree = hir_def::def_query::file_item_tree(db, self.file_text).as_ref()?;
        let def_map = hir_def::def_query::file_def_map(db, self.file_text).as_ref()?;
        let def_data = def_map.0.get(self.raw_def_idx())?;
        let item = *item_tree.top_level_items().get(def_data.item_tree_index)?;
        Some((item_tree, item))
    }

    /// The signature text this definition was written with.
    pub fn signature_text(&self, db: &dyn salsa::Database) -> Option<String> {
        let (tree, item) = self.item(db)?;
        if tree
            .generated
            .get(&item)
            .is_some_and(|origin| origin.kind == generated::Kind::ScatteredEnumVector)
        {
            let head = source_order::at_offset(db, self.file_text, item.name_span(tree).start)?;
            let program = source_order::program(db, self.file_text)
                .unwrap_or(source_order::Program::Inline(self.file_text));
            let vector = source_order::scattered::enum_vectors(db, program)
                .iter()
                .find(|v| v.head == head && v.name == item.name(tree).as_str())?;
            return Some(format!(
                "let {} : vector({}, {})",
                vector.name, vector.members, vector.enum_name
            ));
        }
        Some(item.signature(tree).to_string())
    }

    /// The doc comment attached to this definition.
    pub fn doc_text(&self, db: &dyn salsa::Database) -> Option<String> {
        let (tree, item) = self.item(db)?;
        item.doc(tree).map(str::to_string)
    }

    /// Get the raw DefIdx for DefMap lookups.
    pub fn raw_def_idx(&self) -> DefIdx {
        DefIdx(self.def_id.as_raw())
    }

    /// Get the FileId from the stored FileText.
    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.file_text.file_id(db)
    }

    /// Look up the name of this definition via the database.
    pub fn lookup_name(&self, db: &dyn salsa::Database) -> Name {
        hir_def::def_query::file_def_map(db, self.file_text)
            .as_ref()
            .and_then(|dm| dm.0.get(self.raw_def_idx()))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| Name::new("<unknown>"))
    }

    /// Look up the signature text of this definition from the ItemTree.
    ///
    /// Returns the signature string (e.g. `"int -> bool"`) or `None` if
    /// the item tree is unavailable or the index is out of range.
    pub fn lookup_signature(&self, db: &dyn salsa::Database) -> Option<String> {
        let sig = self.signature_text(db)?;
        if sig.is_empty() {
            None
        } else {
            Some(sig)
        }
    }
}

/// One Sail source file.
///
/// A file is the unit Sail compiles and `$include`s; there is no module
/// system above it and nothing nested inside it, so this is the whole of
/// the containment story. Wraps `FileId` and provides the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct File {
    pub id: base_db::FileId,
}

/// A callable definition (function or mapping clause).
/// Data is accessed via methods that look up the DefMap.
// Difference: id is DefLocation (file+kind) not an interned FunctionId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Function {
    pub(crate) id: DefLocation,
}

impl Function {
    /// Takes `db` instead of `&DefMap``).
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A `type_def` — Sail's one definition form for naming a type.
///
/// The six shapes it can take (`struct`, `union`, `enum`, `bitfield`,
/// `newtype`, `type`) are one definition in Sail's grammar with a
/// discriminant, not six kinds of item, so they are one type here too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeDef {
    pub(crate) id: DefLocation,
    pub(crate) kind: TypeDefKind,
}

/// Which of the six shapes a `type_def` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeDefKind {
    Struct,
    Union,
    Enum,
    Bitfield,
    Newtype,
    TypeAlias,
}

impl TypeDef {
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn kind(&self) -> TypeDefKind {
        self.kind
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.id.file_id(db)
    }

    /// Downcast to `struct`, if that is the form this definition takes.
    pub fn as_struct(&self) -> Option<Struct> {
        (self.kind == TypeDefKind::Struct).then_some(Struct { id: self.id })
    }

    /// Downcast to `union`.
    pub fn as_union(&self) -> Option<Union> {
        (self.kind == TypeDefKind::Union).then_some(Union { id: self.id })
    }

    /// Downcast to `enum`.
    pub fn as_enum(&self) -> Option<Enum> {
        (self.kind == TypeDefKind::Enum).then_some(Enum { id: self.id })
    }
}

impl TypeDefKind {
    pub fn from_item_kind(kind: DefKind) -> Option<Self> {
        match kind {
            DefKind::Struct => Some(Self::Struct),
            DefKind::Union => Some(Self::Union),
            DefKind::Enum => Some(Self::Enum),
            DefKind::Bitfield => Some(Self::Bitfield),
            DefKind::Newtype => Some(Self::Newtype),
            DefKind::TypeAlias => Some(Self::TypeAlias),
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
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
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
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
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
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A register definition (Sail-specific).
// hardware-register name to a type; treated as a distinct DefKind in DefMap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Register {
    pub(crate) id: DefLocation,
}

impl Register {
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
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
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.id.lookup_name(db)
    }

    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.id.file_id(db)
    }
}

/// A `def` — one top-level definition, in Sail's own inventory of them.
///
/// The variants are Sail's `def_aux` forms, not another language's item
/// kinds: a `val` signature is not a function, a scattered clause is not
/// a let binding, and a `termination_measure` is neither. This enum is
/// total over what `DefKind` can classify, so a feature that needs to
/// know which form it is asks the `Def`, not a bare tag alongside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Def {
    /// `function f(x) = ...`, or `function clause f(x) = ...`.
    Function(Function),
    /// `val f : int -> int` — a signature with no body of its own.
    ValSpec(DefLocation),
    /// `struct` / `union` / `enum` / `bitfield` / `newtype` / `type`.
    TypeDef(TypeDef),
    /// `register r : T`.
    Register(Register),
    /// `mapping m : A <-> B`, its clauses, and bare `mapping` signatures.
    Mapping(Mapping),
    /// `let x = ...` or `var x = ...` at top level.
    Let(DefLocation),
    /// `overload f = {g, h}`.
    Overload(DefLocation),
    /// The `scattered function|mapping|union|enum f` that opens a
    /// scattered definition.
    ScatteredHead(DefLocation),
    /// A `union clause` / `enum clause` adding a member to a scattered
    /// definition.
    ScatteredClause(DefLocation),
    /// The `end f` that closes a scattered definition.
    EndMarker(DefLocation),
    /// `constraint T` — a global constraint on the numeric domain.
    Constraint(DefLocation),
    /// `termination_measure f = ...`.
    TerminationMeasure(DefLocation),
    /// `instantiation f with 'a = ...` — instantiating an outcome.
    Instantiation(DefLocation),
    /// `outcome f : 'a -> unit` — declares a callable an `instantiation`
    /// implements.
    Outcome(DefLocation),
    /// One field of a `struct` or `bitfield`. Not top level, but a named
    /// declaration with a location, which is what navigation works in.
    Field(DefLocation),
    /// One member of a `union` or `enum`, declared inline in its braces.
    Variant(DefLocation),
}

impl Def {
    /// Which form this definition takes.
    pub fn kind(&self) -> DefKind {
        match self {
            Self::Function(_) => DefKind::Function,
            Self::ValSpec(_) => DefKind::ValSpec,
            Self::TypeDef(t) => match t.kind {
                TypeDefKind::Struct => DefKind::Struct,
                TypeDefKind::Union => DefKind::Union,
                TypeDefKind::Enum => DefKind::Enum,
                TypeDefKind::Bitfield => DefKind::Bitfield,
                TypeDefKind::Newtype => DefKind::Newtype,
                TypeDefKind::TypeAlias => DefKind::TypeAlias,
            },
            Self::Register(_) => DefKind::Register,
            Self::Mapping(_) => DefKind::Mapping,
            Self::Let(_) => DefKind::Let,
            Self::Overload(_) => DefKind::Overload,
            Self::ScatteredHead(_) => DefKind::ScatteredHead,
            Self::ScatteredClause(_) => DefKind::ScatteredClause,
            Self::EndMarker(_) => DefKind::EndMarker,
            Self::Constraint(_) => DefKind::Constraint,
            Self::TerminationMeasure(_) => DefKind::TerminationMeasure,
            Self::Instantiation(_) => DefKind::Instantiation,
            Self::Outcome(_) => DefKind::Outcome,
            Self::Field(_) => DefKind::Field,
            Self::Variant(_) => DefKind::Variant,
        }
    }

    /// Where this definition is.
    pub fn location(&self) -> DefLocation {
        match self {
            Self::Function(f) => f.id,
            Self::TypeDef(t) => t.id,
            Self::Register(r) => r.id,
            Self::Mapping(m) => m.id,
            Self::ValSpec(loc)
            | Self::Let(loc)
            | Self::Overload(loc)
            | Self::ScatteredHead(loc)
            | Self::ScatteredClause(loc)
            | Self::EndMarker(loc)
            | Self::Constraint(loc)
            | Self::TerminationMeasure(loc)
            | Self::Instantiation(loc)
            | Self::Outcome(loc)
            | Self::Field(loc)
            | Self::Variant(loc) => *loc,
        }
    }

    /// The name this definition declares.
    pub fn name(&self, db: &dyn salsa::Database) -> Name {
        self.location().lookup_name(db)
    }

    /// The file this definition is in.
    pub fn file_id(&self, db: &dyn salsa::Database) -> base_db::FileId {
        self.location().file_id(db)
    }
}

/// What a path or name resolves to.
///
/// Sail has three: a top-level definition, a local binding, and a type
/// variable. There is no self type (no impl blocks), no const parameter,
/// and no attribute or derive machinery to resolve into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathResolution {
    /// Resolved to a top-level definition.
    Def(Def),
    /// Resolved to a local variable/binding.
    Local(Local),
    /// Resolved to a type variable — `'n`, `'m`.
    TypeVar(TypeVar),
}

impl PathResolution {
    /// What the resolver found, as a definition.
    ///
    /// Both of these used to discard their argument: every `TypeNs` arm
    /// came back as `TypeVar` and every `ValueNs` arm as
    /// `Local { name: "", span: 0..0 }`, so `Definition::from_path_resolution`
    /// — the one bridge from name resolution to the IDE's vocabulary —
    /// could not name anything. Whatever asked it got an empty-named local,
    /// which is why nothing above `hir` used `Definition` and every feature
    /// searched by the text under the cursor instead.
    pub fn from_type_ns(
        db: &dyn salsa::Database,
        file_text: base_db::FileText,
        ns: hir_def::resolver::TypeNs,
    ) -> Self {
        use hir_def::resolver::TypeNs;
        match ns {
            TypeNs::BuiltinType(name) => PathResolution::TypeVar(TypeVar { name }),
            // Declared in another file. The name is all this file's
            // resolver knows; the workspace index is what places it.
            TypeNs::Workspace(name) => PathResolution::TypeVar(TypeVar { name }),
            TypeNs::AdtId(id) | TypeNs::TypeAliasId(id) => match def_at_raw(db, file_text, id.0) {
                Some(def) => PathResolution::Def(def),
                None => PathResolution::TypeVar(TypeVar { name: Name::new("") }),
            },
        }
    }

    /// Construct from a `ValueNs` resolution.
    pub fn from_value_ns(
        db: &dyn salsa::Database,
        file_text: base_db::FileText,
        ns: hir_def::resolver::ValueNs,
    ) -> Self {
        use hir_def::resolver::ValueNs;
        let raw = match ns {
            ValueNs::LocalBinding(idx) => Some(idx.0),
            ValueNs::FunctionId(id) => Some(id.0),
            // Sail overloads: a name may be several clauses. They share a
            // name and a declaration site chain, so the first is the one to
            // navigate from and the search finds the rest.
            ValueNs::FunctionIds(ids) => ids.first().map(|id| id.0),
            ValueNs::ValSpecId(id) => Some(id.0),
            ValueNs::RegisterId(id) => Some(id.0),
            ValueNs::MappingId(id) => Some(id.0),
            ValueNs::LetId(id) => Some(id.0),
            ValueNs::EnumVariantId(name) | ValueNs::Workspace(name) => {
                return PathResolution::Local(Local {
                    name,
                    span: Span::new(0, 0),
                    is_param: false,
                })
            }
        };
        match raw.and_then(|raw| def_at_raw(db, file_text, raw)) {
            Some(def) => PathResolution::Def(def),
            None => PathResolution::Local(Local {
                name: Name::new(""),
                span: Span::new(0, 0),
                is_param: false,
            }),
        }
    }
}

/// The definition at a raw `DefMap` index, with the kind the map recorded.
fn def_at_raw(db: &dyn salsa::Database, file_text: base_db::FileText, raw: u32) -> Option<Def> {
    let idx = hir_def::nameres::DefIdx(raw);
    let def_map = hir_def::def_query::file_def_map(db, file_text).as_ref()?;
    let kind = def_map.0.kind_of(idx)?;
    let loc = DefLocation { file_text, def_id: hir_def::DefId::from_def_idx(idx, kind) };
    if kind == DefKind::ScatteredHead {
        if let Some((tree, item)) = loc.item(db) {
            let kind = match item.scattered_type_kind(tree) {
                Some(TypeDefShape::Enum) => Some(TypeDefKind::Enum),
                Some(TypeDefShape::Union) => Some(TypeDefKind::Union),
                _ => None,
            };
            if let Some(kind) = kind {
                return Some(Def::TypeDef(TypeDef { id: loc, kind }));
            }
        }
    }
    Some(Def::from_def_location(loc, kind))
}

/// Recover the explicit definition of a source declaration. Generated rows
/// share its source span but are separate bindings, even with the same name.
pub fn def_at_declaration(
    db: &dyn salsa::Database,
    declaration: source_order::Declaration,
    name: &str,
) -> Option<Def> {
    declaration_definition(db, declaration, name, None)
}

/// Recover a resolved binding without losing its namespace. One declaration
/// can introduce both a type and a same-named generated callable or value.
pub fn def_at_declaration_in_namespace(
    db: &dyn salsa::Database,
    declaration: source_order::Declaration,
    name: &str,
    namespace: source_order::bindings::Namespace,
) -> Option<Def> {
    declaration_definition(db, declaration, name, Some(namespace))
}

fn declaration_definition(
    db: &dyn salsa::Database,
    declaration: source_order::Declaration,
    name: &str,
    namespace: Option<source_order::bindings::Namespace>,
) -> Option<Def> {
    use source_order::bindings::Namespace;
    let node = source_order::node(db, declaration)?;
    let tree = hir_def::def_query::file_item_tree(db, declaration.file).as_ref()?;
    let map = hir_def::def_query::file_def_map(db, declaration.file).as_ref()?;
    map.0.entries().iter().enumerate().find_map(|(raw, data)| {
        let item = tree.top_level_items().get(data.item_tree_index)?;
        if data.name.as_str() != name
            || !node.text_range().contains((item.name_span(tree).start as u32).into())
            || (namespace.is_none() && tree.generated.contains_key(item))
        {
            return None;
        }
        let def = def_at_raw(db, declaration.file, raw as u32)?;
        let callable = matches!(
            def,
            Def::Function(_)
                | Def::ValSpec(_)
                | Def::Mapping(_)
                | Def::Overload(_)
                | Def::ScatteredHead(_)
                | Def::ScatteredClause(_)
                | Def::Outcome(_)
        );
        let matches = match namespace {
            None => true,
            Some(Namespace::Type) => matches!(def, Def::TypeDef(_)),
            Some(Namespace::Callable) => callable,
            Some(Namespace::Value) => callable || matches!(def, Def::Let(_) | Def::Register(_)),
            // Outcomes currently share ValSpec storage in the item tree.
            Some(Namespace::Outcome) => {
                node.kind() == syntax::SyntaxKind::OUTCOME_DEF && !tree.generated.contains_key(item)
            }
        };
        matches.then_some(def)
    })
}

/// A local variable binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Local {
    pub name: Name,
    pub span: Span,
    /// Whether this binding is one of its callable's parameters.
    ///
    /// rust-analyzer's `Local::is_param`. Without it a local renders as
    /// its bare name, which is why `ide/src/hover` could not use
    /// `HirDisplay` for its header: the per-file scrape said "parameter"
    /// or "let binding" and the HIR said nothing.
    pub is_param: bool,
}

/// A type variable — `'n`, `'m`, the things Sail's `typquant` binds.
///
/// Sail's numeric and order variables are the same syntactic category as
/// its type variables, so one type covers all three.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub name: Name,
}
