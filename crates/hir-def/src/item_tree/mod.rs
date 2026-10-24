//! Stable per-file item summary inspired by rust-analyzer's
//! `hir-def::ItemTree`.
//!
//! An [`ItemTree`] is a flat list of every top-level item in a file
//! reduced to its *public surface*: name, kind, signature text. Bodies
//! and initialiser expressions are excluded. The whole tree carries a
//! single `signature_hash` over the rendered surface so two
//! [`ItemTree`]s built from byte-equal source files always hash to
//! the same value, and editing a function body produces a tree that
//! still hashes the same as before — which lets workspace consumers
//! detect "did the public API of this file change?" without going
//! through the typechecker.
//!
//! This is the data layer used by (workspace cache short-circuit
//! when only bodies changed) and as a stable barrier for any future
//! per-item incremental work.
//! is queried via salsa and dependents only invalidate when the
//! tree's identity actually changes.

pub mod generated;
mod lower;

/// The declared fields of a `struct` or `bitfield` node, read once.
///
/// Exported because `hir-ty` builds its `RecordInfo` from the same
/// declaration and used to read it a second way — see
/// `extract_struct_fields_from_cst`.
pub use lower::extract_field_decls;
pub mod pretty;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use la_arena::{Arena, Idx};

use crate::name::Name;
use crate::type_ref::TypeRef;

/// A function or function clause definition.
#[derive(Debug, Clone, Eq)]
pub struct Function {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub is_clause: bool,
    pub member_name: Option<String>,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    /// Per-item signature hash for incremental invalidation.
    pub signature_hash: u64,
}

/// One field of a struct or bitfield, as declared.
#[derive(Debug, Clone, Eq)]
pub struct FieldDecl {
    pub name: Name,
    /// Visibility on this member, independent of the containing declaration.
    pub visibility: crate::visibility::RawVisibility,
    /// Span of the field's name, which is where navigation should land.
    pub name_span: crate::Span,
    /// The declared type as written (`Access_variety`, `0 .. 3`), for the
    /// detail line. Empty when it could not be read off the source.
    pub type_text: String,
}

/// A type definition (struct, union, enum, bitfield, newtype, type alias).
#[derive(Debug, Clone, Eq)]
pub struct TypeDef {
    pub name: Name,
    pub kind: TypeDefKind,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    /// The names this type declares members under: a struct's or bitfield's
    /// fields. Empty for the kinds that have none.
    ///
    /// A field is not a top-level item and never appears in
    /// `top_level_items()`, but it is a declaration with a name and a place
    /// in the source, which is all navigation needs. rust-analyzer carries
    /// fields in its symbol index for the same reason.
    pub fields: Vec<FieldDecl>,
    pub is_clause: bool,
    pub member_name: Option<String>,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    pub signature_hash: u64,
}

/// Sub-kind for type definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeDefKind {
    Struct,
    Union,
    Enum,
    Bitfield,
    Newtype,
    TypeAlias,
}

/// A register definition.
#[derive(Debug, Clone, Eq)]
pub struct Register {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// A `val` specification (type signature).
#[derive(Debug, Clone, Eq)]
pub struct ValSpec {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    pub signature_hash: u64,
}

/// A mapping or mapping clause.
#[derive(Debug, Clone, Eq)]
pub struct Mapping {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub is_clause: bool,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    pub signature_hash: u64,
}

/// A top-level `let` or `var` binding.
#[derive(Debug, Clone, Eq)]
pub struct LetDef {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub is_var: bool,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// An `overload` declaration.
#[derive(Debug, Clone, Eq)]
pub struct Overload {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub visibility: crate::visibility::RawVisibility,
}

/// A scattered definition head or clause.
#[derive(Debug, Clone, Eq)]
pub struct ScatteredDef {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub is_head: bool,
    /// For union/enum clauses: the member name (e.g. "ADD" in `union clause op = ADD`).
    pub member_name: Option<String>,
    /// Where that member's name is written. The clause is indexed under the
    /// scattered type's name (`op`), so without this the member itself —
    /// which is what the source refers to everywhere else — has no location.
    pub member_span: Option<crate::Span>,
    pub fields: Vec<FieldDecl>,
    pub type_kind: Option<TypeDefKind>,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// A constraint definition.
#[derive(Debug, Clone, Eq)]
pub struct Constraint {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub visibility: crate::visibility::RawVisibility,
}

/// A termination measure or end marker.
#[derive(Debug, Clone, Eq)]
pub struct Pragma {
    pub name: Name,
    pub text: String,
    pub span: crate::Span,
    /// Span of just the declared name, for cursor placement.
    pub name_span: crate::Span,
    pub pragma_kind: PragmaKind,
    pub visibility: crate::visibility::RawVisibility,
}

/// Sub-kind for pragmas (things that aren't real definitions but appear in the tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PragmaKind {
    TerminationMeasure,
    EndMarker,
    /// `instantiation foo with 'a = bar_kind` — outcome instantiation.
    Instantiation,
    MappingSpec,
}

/// An item in the item tree, identified by which arena it lives in
/// and its index within that arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefItem {
    Function(Idx<Function>),
    TypeDef(Idx<TypeDef>),
    Register(Idx<Register>),
    ValSpec(Idx<ValSpec>),
    Mapping(Idx<Mapping>),
    Let(Idx<LetDef>),
    Overload(Idx<Overload>),
    Scattered(Idx<ScatteredDef>),
    Constraint(Idx<Constraint>),
    Pragma(Idx<Pragma>),
}

impl DefItem {
    /// Get the name of the item, if it has one (all items have names).
    pub fn name<'a>(&self, tree: &'a ItemTree) -> &'a Name {
        match self {
            Self::Function(id) => &tree.functions[*id].name,
            Self::TypeDef(id) => &tree.type_defs[*id].name,
            Self::Register(id) => &tree.registers[*id].name,
            Self::ValSpec(id) => &tree.val_specs[*id].name,
            Self::Mapping(id) => &tree.mappings[*id].name,
            Self::Let(id) => &tree.lets[*id].name,
            Self::Overload(id) => &tree.overloads[*id].name,
            Self::Scattered(id) => &tree.scattered[*id].name,
            Self::Constraint(id) => &tree.constraints[*id].name,
            Self::Pragma(id) => &tree.pragmas[*id].name,
        }
    }

    /// Get the span of the item.
    /// Span of just the declared name.
    ///
    /// Where a cursor should land when navigating to this item;
    /// [`DefItemId::span`] covers the whole definition.
    pub fn name_span(&self, tree: &ItemTree) -> crate::Span {
        match self {
            Self::Function(id) => tree.functions[*id].name_span,
            Self::TypeDef(id) => tree.type_defs[*id].name_span,
            Self::Register(id) => tree.registers[*id].name_span,
            Self::ValSpec(id) => tree.val_specs[*id].name_span,
            Self::Mapping(id) => tree.mappings[*id].name_span,
            Self::Let(id) => tree.lets[*id].name_span,
            Self::Overload(id) => tree.overloads[*id].name_span,
            Self::Scattered(id) => tree.scattered[*id].name_span,
            Self::Constraint(id) => tree.constraints[*id].name_span,
            Self::Pragma(id) => tree.pragmas[*id].name_span,
        }
    }

    pub fn span(&self, tree: &ItemTree) -> crate::Span {
        match self {
            Self::Function(id) => tree.functions[*id].span,
            Self::TypeDef(id) => tree.type_defs[*id].span,
            Self::Register(id) => tree.registers[*id].span,
            Self::ValSpec(id) => tree.val_specs[*id].span,
            Self::Mapping(id) => tree.mappings[*id].span,
            Self::Let(id) => tree.lets[*id].span,
            Self::Overload(id) => tree.overloads[*id].span,
            Self::Scattered(id) => tree.scattered[*id].span,
            Self::Constraint(id) => tree.constraints[*id].span,
            Self::Pragma(id) => tree.pragmas[*id].span,
        }
    }

    /// Get the signature text of the item.
    pub fn signature<'a>(&self, tree: &'a ItemTree) -> &'a str {
        match self {
            Self::Function(id) => &tree.functions[*id].signature,
            Self::TypeDef(id) => &tree.type_defs[*id].signature,
            Self::Register(id) => &tree.registers[*id].signature,
            Self::ValSpec(id) => &tree.val_specs[*id].signature,
            Self::Mapping(id) => &tree.mappings[*id].signature,
            Self::Let(id) => &tree.lets[*id].signature,
            Self::Overload(id) => &tree.overloads[*id].signature,
            Self::Scattered(id) => &tree.scattered[*id].signature,
            Self::Constraint(id) => &tree.constraints[*id].signature,
            Self::Pragma(id) => &tree.pragmas[*id].text,
        }
    }

    /// Get the doc comment of the item, if any.
    pub fn doc<'a>(&self, tree: &'a ItemTree) -> Option<&'a str> {
        match self {
            Self::Function(id) => tree.functions[*id].doc.as_deref(),
            Self::TypeDef(id) => tree.type_defs[*id].doc.as_deref(),
            Self::Register(id) => tree.registers[*id].doc.as_deref(),
            Self::ValSpec(id) => tree.val_specs[*id].doc.as_deref(),
            Self::Mapping(id) => tree.mappings[*id].doc.as_deref(),
            Self::Let(id) => tree.lets[*id].doc.as_deref(),
            Self::Scattered(id) => tree.scattered[*id].doc.as_deref(),
            Self::Overload(_) => tree.generated.get(self).and_then(|origin| origin.owner.doc(tree)),
            Self::Constraint(_) | Self::Pragma(_) => None,
        }
    }

    /// The members this item declares under their own names — a struct's or
    /// bitfield's fields. Empty for everything else.
    pub fn fields<'a>(&self, tree: &'a ItemTree) -> &'a [FieldDecl] {
        match self {
            Self::TypeDef(id) => &tree.type_defs[*id].fields,
            Self::Scattered(id) => &tree.scattered[*id].fields,
            _ => &[],
        }
    }

    pub fn scattered_type_kind(&self, tree: &ItemTree) -> Option<TypeDefKind> {
        match self {
            Self::Scattered(id) => tree.scattered[*id].type_kind,
            _ => None,
        }
    }

    /// The member a scattered `union clause`/`enum clause` declares, with the
    /// span of its name. `enum clause extension = Ext_S` is indexed under
    /// `extension`, so `Ext_S` — the name every use site writes — needs its
    /// own entry.
    pub fn scattered_member<'a>(&self, tree: &'a ItemTree) -> Option<(&'a str, crate::Span)> {
        match self {
            Self::Scattered(id) => {
                let d = &tree.scattered[*id];
                Some((d.member_name.as_deref()?, d.member_span?))
            }
            _ => None,
        }
    }

    /// Map to the legacy `DefKind` for backward compatibility.
    pub fn def_kind(&self, tree: &ItemTree) -> DefKind {
        match self {
            // is_clause does not affect the legacy DefKind mapping.
            Self::Function(_) => DefKind::Function,
            Self::TypeDef(id) => match tree.type_defs[*id].kind {
                TypeDefKind::Struct => DefKind::Struct,
                TypeDefKind::Union => DefKind::Union,
                TypeDefKind::Enum => DefKind::Enum,
                TypeDefKind::Bitfield => DefKind::Bitfield,
                TypeDefKind::Newtype => DefKind::Newtype,
                TypeDefKind::TypeAlias => DefKind::TypeAlias,
            },
            Self::Register(_) => DefKind::Register,
            Self::ValSpec(_) => DefKind::ValSpec,
            Self::Mapping(_) => DefKind::Mapping,
            Self::Let(id) => {
                if tree.lets[*id].is_var {
                    DefKind::Var
                } else {
                    DefKind::Let
                }
            }
            Self::Overload(_) => DefKind::Overload,
            Self::Scattered(id) => {
                if tree.scattered[*id].is_head {
                    DefKind::ScatteredHead
                } else {
                    DefKind::ScatteredClause
                }
            }
            Self::Constraint(_) => DefKind::Constraint,
            Self::Pragma(id) => match tree.pragmas[*id].pragma_kind {
                PragmaKind::TerminationMeasure => DefKind::TerminationMeasure,
                PragmaKind::EndMarker => DefKind::EndMarker,
                PragmaKind::MappingSpec => DefKind::MappingSpec,
                PragmaKind::Instantiation => DefKind::Instantiation,
            },
        }
    }

    /// Whether this is a `clause` form (e.g., `function clause foo`).
    pub fn is_clause(&self, tree: &ItemTree) -> bool {
        match self {
            Self::Function(id) => tree.functions[*id].is_clause,
            Self::TypeDef(id) => tree.type_defs[*id].is_clause,
            Self::Mapping(id) => tree.mappings[*id].is_clause,
            _ => false,
        }
    }

    /// For scattered union/enum clauses: the member name.
    pub fn member_name<'a>(&self, tree: &'a ItemTree) -> Option<&'a str> {
        match self {
            Self::Function(id) => tree.functions[*id].member_name.as_deref(),
            Self::TypeDef(id) => tree.type_defs[*id].member_name.as_deref(),
            Self::Scattered(id) => tree.scattered[*id].member_name.as_deref(),
            _ => None,
        }
    }

    /// Get the type reference of this item (if available).
    pub fn type_ref<'a>(&self, tree: &'a ItemTree) -> Option<&'a TypeRef> {
        match self {
            Self::Function(id) => tree.functions[*id].type_ref.as_ref(),
            Self::TypeDef(id) => tree.type_defs[*id].type_ref.as_ref(),
            Self::Register(id) => tree.registers[*id].type_ref.as_ref(),
            Self::ValSpec(id) => tree.val_specs[*id].type_ref.as_ref(),
            Self::Mapping(id) => tree.mappings[*id].type_ref.as_ref(),
            Self::Let(id) => tree.lets[*id].type_ref.as_ref(),
            Self::Scattered(id) => tree.scattered[*id].type_ref.as_ref(),
            Self::Constraint(id) => tree.constraints[*id].type_ref.as_ref(),
            _ => None,
        }
    }

    /// Visibility of this item.
    pub fn visibility(&self, tree: &ItemTree) -> crate::visibility::RawVisibility {
        match self {
            Self::Function(id) => tree.functions[*id].visibility,
            Self::TypeDef(id) => tree.type_defs[*id].visibility,
            Self::Register(id) => tree.registers[*id].visibility,
            Self::ValSpec(id) => tree.val_specs[*id].visibility,
            Self::Mapping(id) => tree.mappings[*id].visibility,
            Self::Let(id) => tree.lets[*id].visibility,
            Self::Overload(id) => tree.overloads[*id].visibility,
            Self::Scattered(id) => tree.scattered[*id].visibility,
            Self::Constraint(id) => tree.constraints[*id].visibility,
            Self::Pragma(id) => tree.pragmas[*id].visibility,
        }
    }
}

// Identity, and not position.
//
// rust-analyzer's `ItemTree` header calls this layer "an invalidation
// barrier for incremental computations: when typing inside an item body,
// the `ItemTree` of the modified file is typically unaffected, so we don't
// have to recompute name resolution results". Its items carry an `AstId`
// and no offsets, so that holds by construction. Ours carry spans —
// navigation reads them straight off the tree rather than through a second
// query — so it has to hold by definition instead: two items that say the
// same thing are the same item, wherever they sit.
//
// `file_def_map`'s only input is `file_item_tree`, so this is precisely
// what decides whether editing a body redoes name resolution for the file.
// `the_same_items_at_a_different_offset_are_the_same_tree` pins it.
//
// Each impl destructures exhaustively: a field added later fails to
// compile here rather than silently dropping out of the comparison, which
// would leave name resolution stale for whatever it holds.

impl PartialEq for Function {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            signature,
            type_ref,
            span: _,
            name_span: _,
            is_clause,
            member_name,
            doc,
            visibility,
            signature_hash,
        } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            is_clause: o_is_clause,
            member_name: o_member_name,
            doc: o_doc,
            visibility: o_visibility,
            signature_hash: o_signature_hash,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && is_clause == o_is_clause
            && member_name == o_member_name
            && doc == o_doc
            && visibility == o_visibility
            && signature_hash == o_signature_hash
    }
}

impl PartialEq for FieldDecl {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, visibility, name_span: _, type_text } = self;
        let Self { name: o_name, visibility: o_visibility, name_span: _, type_text: o_type_text } =
            other;
        name == o_name && visibility == o_visibility && type_text == o_type_text
    }
}

impl PartialEq for TypeDef {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            kind,
            signature,
            type_ref,
            span: _,
            name_span: _,
            fields,
            is_clause,
            member_name,
            doc,
            visibility,
            signature_hash,
        } = self;
        let Self {
            name: o_name,
            kind: o_kind,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            fields: o_fields,
            is_clause: o_is_clause,
            member_name: o_member_name,
            doc: o_doc,
            visibility: o_visibility,
            signature_hash: o_signature_hash,
        } = other;
        name == o_name
            && kind == o_kind
            && signature == o_signature
            && type_ref == o_type_ref
            && fields == o_fields
            && is_clause == o_is_clause
            && member_name == o_member_name
            && doc == o_doc
            && visibility == o_visibility
            && signature_hash == o_signature_hash
    }
}

impl PartialEq for Register {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, signature, type_ref, span: _, name_span: _, doc, visibility } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            doc: o_doc,
            visibility: o_visibility,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && doc == o_doc
            && visibility == o_visibility
    }
}

impl PartialEq for ValSpec {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            signature,
            type_ref,
            span: _,
            name_span: _,
            doc,
            visibility,
            signature_hash,
        } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            doc: o_doc,
            visibility: o_visibility,
            signature_hash: o_signature_hash,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && doc == o_doc
            && visibility == o_visibility
            && signature_hash == o_signature_hash
    }
}

impl PartialEq for Mapping {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            signature,
            type_ref,
            span: _,
            name_span: _,
            is_clause,
            doc,
            visibility,
            signature_hash,
        } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            is_clause: o_is_clause,
            doc: o_doc,
            visibility: o_visibility,
            signature_hash: o_signature_hash,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && is_clause == o_is_clause
            && doc == o_doc
            && visibility == o_visibility
            && signature_hash == o_signature_hash
    }
}

impl PartialEq for LetDef {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, signature, type_ref, span: _, name_span: _, is_var, doc, visibility } =
            self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            is_var: o_is_var,
            doc: o_doc,
            visibility: o_visibility,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && is_var == o_is_var
            && doc == o_doc
            && visibility == o_visibility
    }
}

impl PartialEq for Overload {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, signature, type_ref, span: _, name_span: _, visibility } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            visibility: o_visibility,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && visibility == o_visibility
    }
}

impl PartialEq for ScatteredDef {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            signature,
            type_ref,
            span: _,
            name_span: _,
            is_head,
            member_name,
            member_span: _,
            fields,
            type_kind,
            doc,
            visibility,
        } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            is_head: o_is_head,
            member_name: o_member_name,
            member_span: _,
            fields: o_fields,
            type_kind: o_type_kind,
            doc: o_doc,
            visibility: o_visibility,
        } = other;
        name == o_name
            && fields == o_fields
            && type_kind == o_type_kind
            && signature == o_signature
            && type_ref == o_type_ref
            && is_head == o_is_head
            && member_name == o_member_name
            && doc == o_doc
            && visibility == o_visibility
    }
}

impl PartialEq for Constraint {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, signature, type_ref, span: _, name_span: _, visibility } = self;
        let Self {
            name: o_name,
            signature: o_signature,
            type_ref: o_type_ref,
            span: _,
            name_span: _,
            visibility: o_visibility,
        } = other;
        name == o_name
            && signature == o_signature
            && type_ref == o_type_ref
            && visibility == o_visibility
    }
}

impl PartialEq for Pragma {
    fn eq(&self, other: &Self) -> bool {
        let Self { name, text, span: _, name_span: _, pragma_kind, visibility } = self;
        let Self {
            name: o_name,
            text: o_text,
            span: _,
            name_span: _,
            pragma_kind: o_pragma_kind,
            visibility: o_visibility,
        } = other;
        name == o_name
            && text == o_text
            && pragma_kind == o_pragma_kind
            && visibility == o_visibility
    }
}

impl PartialEq for ItemTree {
    fn eq(&self, other: &Self) -> bool {
        // `include_spans` is where the file's `$include`s are, which moves
        // with everything else; the paths themselves are the identity.
        let Self {
            signature_hash,
            fixities,
            include_spans,
            top_level,
            functions,
            type_defs,
            registers,
            val_specs,
            mappings,
            lets,
            overloads,
            scattered,
            constraints,
            pragmas,
            generated,
        } = self;
        let includes = |v: &Vec<(String, crate::Span)>| {
            v.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>()
        };
        *signature_hash == other.signature_hash
            && *fixities == other.fixities
            && includes(include_spans) == includes(&other.include_spans)
            && *top_level == other.top_level
            && *functions == other.functions
            && *type_defs == other.type_defs
            && *registers == other.registers
            && *val_specs == other.val_specs
            && *mappings == other.mappings
            && *lets == other.lets
            && *overloads == other.overloads
            && *scattered == other.scattered
            && *constraints == other.constraints
            && *pragmas == other.pragmas
            && *generated == other.generated
    }
}

#[derive(Clone, Debug, Eq)]
pub struct ItemTree {
    /// Legacy: one entry per public top-level item, in source order.
    /// Combined hash over all entries' rendered signature text.
    pub signature_hash: u64,
    /// Fixity declarations (`infixl N op`, `infixr N op`, `infix N op`).
    pub fixities: Vec<FixityDecl>,
    /// Spans of `$include` directives in this file.
    /// Each entry is (include_path_text, span_of_directive).
    /// So an unresolved `$include` can be reported at the directive.
    pub include_spans: Vec<(String, crate::Span)>,

    /// Top-level items in source order, indexing into typed arenas.
    pub top_level: Vec<DefItem>,
    /// Synthetic names retain their generating item instead of pretending
    /// that its name token spells the synthetic name.
    pub generated: std::collections::HashMap<DefItem, generated::Origin>,
    /// Per-kind arenas.
    pub(crate) functions: Arena<Function>,
    pub(crate) type_defs: Arena<TypeDef>,
    pub(crate) registers: Arena<Register>,
    pub(crate) val_specs: Arena<ValSpec>,
    pub(crate) mappings: Arena<Mapping>,
    pub(crate) lets: Arena<LetDef>,
    pub(crate) overloads: Arena<Overload>,
    pub(crate) scattered: Arena<ScatteredDef>,
    pub(crate) constraints: Arena<Constraint>,
    pub(crate) pragmas: Arena<Pragma>,
}

/// A fixity declaration from the source file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixityDecl {
    /// The operator name (e.g., `+`, `*`, or a custom operator).
    pub operator: String,
    /// Precedence level (0-9).
    pub level: u8,
    /// Associativity.
    pub assoc: Associativity,
}

/// Operator associativity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Associativity {
    /// `infix N op` — non-associative.
    None,
    /// `infixl N op` — left-associative.
    Left,
    /// `infixr N op` — right-associative.
    Right,
}

/// Coarse classification of a top-level item, modelled after upstream
/// Sail's `def_aux` constructors plus the structural categories
/// sail-lsp's typechecker cares about. Items that are pure
/// metadata (`$pragma`, `$attribute`, `default`, `infix N`,
/// `instantiation`, `end`) are intentionally excluded from the
/// `ItemTree` since they don't form part of the file's public
/// surface in the sense that downstream typechecking depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefKind {
    /// `function foo(...) = ...` or `function clause foo(...) = ...`
    Function,
    /// `mapping foo : ... = { ... }` or `mapping clause foo = ...`
    Mapping,
    /// `val foo : ...` (signature only).
    ValSpec,
    /// `mapping foo : ...` (signature only).
    MappingSpec,
    /// `type foo = ...`
    TypeAlias,
    /// `struct foo = { ... }`
    Struct,
    /// `union foo = { ... }`
    Union,
    /// `enum foo = { ... }`
    Enum,
    /// `bitfield foo : bits(N) = { ... }`
    Bitfield,
    /// `newtype foo = ...`
    Newtype,
    /// `register foo : T`
    Register,
    /// `let x = ...` at top level (only the type signature is hashed).
    Let,
    /// `var x = ...` at top level (only the type signature is hashed).
    Var,
    /// `overload foo = { ... }`
    Overload,
    /// `scattered function|mapping|union|enum foo : ...` head.
    ScatteredHead,
    /// `union clause Name = ...` or `enum clause Name = ...`.
    ScatteredClause,
    /// `constraint T` or `type constraint T`.
    Constraint,
    /// `termination_measure foo = ...`
    TerminationMeasure,
    /// `end foo` — scattered definition end marker.
    EndMarker,
    /// `instantiation foo with 'a = bar_kind` — outcome instantiation.
    Instantiation,
    /// `outcome foo : 'a -> unit` — Sail's `DEF_outcome`. Declares a
    /// callable whose implementation an `instantiation` supplies.
    Outcome,
    /// One field of a `struct` or `bitfield`.
    ///
    /// Not a top-level item — nothing in `top_level_items()` ever carries
    /// this — but a named declaration with a source location, which is what
    /// the symbol index and navigation work in terms of.
    Field,
    /// One member of a `union` or `enum`, declared inline in its braces.
    /// A member declared as `enum clause E = A` is a `ScatteredClause`
    /// instead, and carries the member's own name in `member_name`.
    Variant,
}

impl DefKind {
    /// The keyword a definition of this kind is written with.
    ///
    /// `None` for the two kinds that are not written as a keyword-led
    /// definition: a struct or bitfield field, and an inline union or
    /// enum member.
    pub fn keyword(self) -> Option<&'static str> {
        Some(match self {
            Self::Function => "function",
            Self::Mapping | Self::MappingSpec => "mapping",
            Self::ValSpec => "val",
            Self::TypeAlias => "type",
            Self::Struct => "struct",
            Self::Union => "union",
            Self::Enum => "enum",
            Self::Bitfield => "bitfield",
            Self::Newtype => "newtype",
            Self::Register => "register",
            Self::Let => "let",
            Self::Var => "var",
            Self::Overload => "overload",
            Self::ScatteredHead => "scattered",
            Self::ScatteredClause => "clause",
            Self::Constraint => "constraint",
            Self::TerminationMeasure => "termination_measure",
            Self::EndMarker => "end",
            Self::Instantiation => "instantiation",
            Self::Outcome => "outcome",
            Self::Field | Self::Variant => return None,
        })
    }
}

impl ItemTree {
    /// Top-level items in source order (typed arena version).
    pub fn top_level_items(&self) -> &[DefItem] {
        &self.top_level
    }

    /// Number of top-level items. Replaces `entries.len()`.
    pub fn len(&self) -> usize {
        self.top_level.len()
    }

    /// Whether the tree is empty. Replaces `entries.is_empty()`.
    pub fn is_empty(&self) -> bool {
        self.top_level.is_empty()
    }

    pub fn function(&self, id: Idx<Function>) -> &Function {
        &self.functions[id]
    }
    pub fn type_def(&self, id: Idx<TypeDef>) -> &TypeDef {
        &self.type_defs[id]
    }
    pub fn register(&self, id: Idx<Register>) -> &Register {
        &self.registers[id]
    }
    pub fn val_spec(&self, id: Idx<ValSpec>) -> &ValSpec {
        &self.val_specs[id]
    }
    pub fn mapping(&self, id: Idx<Mapping>) -> &Mapping {
        &self.mappings[id]
    }
    pub fn let_def(&self, id: Idx<LetDef>) -> &LetDef {
        &self.lets[id]
    }
    pub fn overload(&self, id: Idx<Overload>) -> &Overload {
        &self.overloads[id]
    }
    pub fn scattered_def(&self, id: Idx<ScatteredDef>) -> &ScatteredDef {
        &self.scattered[id]
    }
    pub fn constraint(&self, id: Idx<Constraint>) -> &Constraint {
        &self.constraints[id]
    }
    pub fn pragma(&self, id: Idx<Pragma>) -> &Pragma {
        &self.pragmas[id]
    }

    /// Create an empty ItemTree (used internally).
    #[allow(deprecated)]
    fn empty() -> Self {
        Self {
            signature_hash: 0,
            fixities: Vec::new(),
            include_spans: Vec::new(),
            top_level: Vec::new(),
            generated: Default::default(),
            functions: Arena::new(),
            type_defs: Arena::new(),
            registers: Arena::new(),
            val_specs: Arena::new(),
            mappings: Arena::new(),
            lets: Arena::new(),
            overloads: Arena::new(),
            scattered: Arena::new(),
            constraints: Arena::new(),
            pragmas: Arena::new(),
        }
    }

    pub fn find_by_name(&self, name: &str) -> Option<DefItem> {
        self.top_level.iter().copied().find(|id| id.name(self).as_str() == name)
    }

    /// Fill in per-item `signature_hash` fields from name + signature text.
    /// Called once after construction. Replaces the single tree-level hash
    /// with per-item granularity for better incremental invalidation.
    pub fn compute_per_item_hashes(&mut self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let hash = |kind: &str, name: &str, sig: &str| -> u64 {
            let mut h = DefaultHasher::new();
            kind.hash(&mut h);
            name.hash(&mut h);
            sig.hash(&mut h);
            h.finish()
        };

        for (_, f) in self.functions.iter_mut() {
            f.signature_hash = hash("Function", f.name.as_str(), &f.signature);
        }
        for (_, t) in self.type_defs.iter_mut() {
            t.signature_hash = hash("TypeDef", t.name.as_str(), &t.signature);
        }
        for (_, v) in self.val_specs.iter_mut() {
            v.signature_hash = hash("ValSpec", v.name.as_str(), &v.signature);
        }
        for (_, m) in self.mappings.iter_mut() {
            m.signature_hash = hash("Mapping", m.name.as_str(), &m.signature);
        }

        // Recompute tree-level hash as XOR of all per-item hashes.
        let mut combined = 0u64;
        for (_, f) in self.functions.iter() {
            combined ^= f.signature_hash;
        }
        for (_, t) in self.type_defs.iter() {
            combined ^= t.signature_hash;
        }
        for (_, v) in self.val_specs.iter() {
            combined ^= v.signature_hash;
        }
        for (_, m) in self.mappings.iter() {
            combined ^= m.signature_hash;
        }
        self.signature_hash = combined;
    }

    /// Build a fixity context (operator→binding power map) from this tree's fixity declarations.
    /// Used by the parser for dynamic operator precedence.
    pub fn build_fixity_context(&self) -> syntax::FixityContext {
        let mut ctx = syntax::FixityContext::new();
        for decl in &self.fixities {
            let base_bp = decl.level * 2 + 1;
            let (l_bp, r_bp) = match decl.assoc {
                Associativity::Left => (base_bp, base_bp + 1),
                Associativity::Right => (base_bp + 1, base_bp),
                Associativity::None => (base_bp, base_bp),
            };
            ctx.insert(decl.operator.clone(), (l_bp, r_bp));
        }
        ctx
    }

    /// Iterator over every top-level item of a given kind.
    pub fn items_of_kind(&self, kind: DefKind) -> impl Iterator<Item = DefItem> + '_ {
        self.top_level.iter().copied().filter(move |id| id.def_kind(self) == kind)
    }
}

/// Legacy: compute signature hash directly from typed arenas.
/// Superseded by `ItemTree::compute_per_item_hashes` .
#[expect(dead_code)]
fn compute_signature_hash(tree: &ItemTree) -> u64 {
    let mut hasher = DefaultHasher::new();
    tree.top_level.len().hash(&mut hasher);
    for id in &tree.top_level {
        id.def_kind(tree).hash(&mut hasher);
        id.name(tree).hash(&mut hasher);
        id.signature(tree).hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod test_visibility_detection {
    use super::*;

    #[test]
    fn detects_dollar_bracket_private() {
        let source = "$[private] function secret() = 42\nfunction public_fn() = 0\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let items = tree.top_level_items();
        assert!(items.len() >= 2, "should have at least 2 items");
        assert_eq!(
            items[0].visibility(&tree),
            crate::visibility::RawVisibility::Private,
            "first item should be Private"
        );
        assert_eq!(
            items[1].visibility(&tree),
            crate::visibility::RawVisibility::Public,
            "second item should be Public"
        );
    }

    /// Comma-separated attributes `$[private, other]` should
    /// correctly detect private visibility.
    ///
    /// Sail allows:
    /// ```sail
    /// $[private, deprecated]
    /// function secret() = 42
    /// ```
    #[test]
    fn detects_comma_separated_private_attribute() {
        let source = "$[private, deprecated] function secret() = 42\nfunction public_fn() = 0\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let items = tree.top_level_items();
        assert!(items.len() >= 2, "should have at least 2 items");
        assert_eq!(
            items[0].visibility(&tree),
            crate::visibility::RawVisibility::Private,
            "comma-separated $[private, deprecated] should be Private"
        );
        assert_eq!(
            items[1].visibility(&tree),
            crate::visibility::RawVisibility::Public,
            "second item should be Public"
        );
    }

    /// `$[other, private]` — private not first in the list.
    #[test]
    fn detects_private_not_first_in_comma_list() {
        let source = "$[deprecated, private] function secret() = 42\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let items = tree.top_level_items();
        assert!(!items.is_empty());
        assert_eq!(
            items[0].visibility(&tree),
            crate::visibility::RawVisibility::Private,
            "$[deprecated, private] should still be Private"
        );
    }

    /// Non-private attribute should remain Public.
    #[test]
    fn non_private_comma_attribute_stays_public() {
        let source = "$[deprecated, experimental] function f() = 0\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let items = tree.top_level_items();
        assert!(!items.is_empty());
        assert_eq!(
            items[0].visibility(&tree),
            crate::visibility::RawVisibility::Public,
            "$[deprecated, experimental] should be Public"
        );
    }
}

#[cfg(test)]
mod position_independence {
    use super::*;

    /// The same items, written at two different offsets, are the same tree.
    ///
    /// This is the invariant rust-analyzer's `ItemTree` states in its own
    /// header: the layer exists "to provide an invalidation barrier for
    /// incremental computations", so that typing inside a body does not
    /// recompute name resolution. It holds only if the tree's *identity*
    /// carries no position — RA's items hold an `AstId`, not a span.
    ///
    /// Written as a comparison rather than a field audit because it catches
    /// the next position field too, wherever someone adds it.
    #[test]
    fn the_same_items_at_a_different_offset_are_the_same_tree() {
        let body = "\
type xlenbits = bits(64)
val f : int -> int
function f(x) = x + 1
struct S = { a : int, b : bool }
$include \"other.sail\"
scattered function g
end g
";
        let build = |src: &str| {
            let (root, _) = syntax::parse_text(src);
            ItemTree::build_from_cst(&root)
        };
        let here = build(body);
        // Everything moved down two lines and right by a comment.
        let moved = build(&format!("// a comment nobody asked about\n\n{body}"));
        assert_eq!(here, moved, "the tree changed because the items moved");
    }
}
