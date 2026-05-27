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

mod lower;
pub mod pretty;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use la_arena::{Arena, Idx};

use crate::name::Name;
use crate::type_ref::TypeRef;

/// A function or function clause definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub is_clause: bool,
    pub member_name: Option<String>,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    /// Per-item signature hash for incremental invalidation.
    pub signature_hash: u64,
}

/// A type definition (struct, union, enum, bitfield, newtype, type alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDef {
    pub name: Name,
    pub kind: TypeDefKind,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// A `val` specification (type signature).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValSpec {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    pub signature_hash: u64,
}

/// A mapping or mapping clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub is_clause: bool,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
    pub signature_hash: u64,
}

/// A top-level `let` or `var` binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetDef {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub is_var: bool,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// An `overload` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overload {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub visibility: crate::visibility::RawVisibility,
}

/// A scattered definition head or clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScatteredDef {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub is_head: bool,
    /// For union/enum clauses: the member name (e.g. "ADD" in `union clause op = ADD`).
    pub member_name: Option<String>,
    pub doc: Option<String>,
    pub visibility: crate::visibility::RawVisibility,
}

/// A constraint definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: Name,
    pub signature: String,
    pub type_ref: Option<TypeRef>,
    pub span: crate::Span,
    pub visibility: crate::visibility::RawVisibility,
}

/// A termination measure or end marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pragma {
    pub name: Name,
    pub text: String,
    pub span: crate::Span,
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
pub enum ModItem {
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

impl ModItem {
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
            Self::Overload(_) | Self::Constraint(_) | Self::Pragma(_) => None,
        }
    }

    /// Map to the legacy `ItemKind` for backward compatibility.
    pub fn item_kind(&self, tree: &ItemTree) -> ItemKind {
        match self {
            Self::Function(id) => {
                if tree.functions[*id].is_clause {
                    ItemKind::Function // still Function, is_clause is separate
                } else {
                    ItemKind::Function
                }
            }
            Self::TypeDef(id) => match tree.type_defs[*id].kind {
                TypeDefKind::Struct => ItemKind::Struct,
                TypeDefKind::Union => ItemKind::Union,
                TypeDefKind::Enum => ItemKind::Enum,
                TypeDefKind::Bitfield => ItemKind::Bitfield,
                TypeDefKind::Newtype => ItemKind::Newtype,
                TypeDefKind::TypeAlias => ItemKind::TypeAlias,
            },
            Self::Register(_) => ItemKind::Register,
            Self::ValSpec(_) => ItemKind::ValSpec,
            Self::Mapping(_) => ItemKind::Mapping,
            Self::Let(id) => {
                if tree.lets[*id].is_var {
                    ItemKind::Var
                } else {
                    ItemKind::Let
                }
            }
            Self::Overload(_) => ItemKind::Overload,
            Self::Scattered(id) => {
                if tree.scattered[*id].is_head {
                    ItemKind::ScatteredHead
                } else {
                    ItemKind::ScatteredClause
                }
            }
            Self::Constraint(_) => ItemKind::Constraint,
            Self::Pragma(id) => match tree.pragmas[*id].pragma_kind {
                PragmaKind::TerminationMeasure => ItemKind::TerminationMeasure,
                PragmaKind::EndMarker => ItemKind::EndMarker,
                PragmaKind::MappingSpec => ItemKind::MappingSpec,
                PragmaKind::Instantiation => ItemKind::Instantiation,
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

/// Stable summary of every top-level item in a file. Constructed via
/// [`ItemTree::build`]. Cheap to clone (just two fields), cheap to
/// equality-compare (by hash), and stable across edits to function
/// bodies / initialiser values.
///
/// # Typed Arena Architecture
///
/// One item in an [`ItemTree`]. Lossy by design: holds only the
/// public surface needed to compare across revisions, not the full
/// AST node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemTreeEntry {
    pub name: Name,
    pub kind: ItemKind,
    /// Canonical span-free rendering of this item's signature.
    pub signature_text: String,
    /// Per-entry hash over `(kind, name, signature_text)`.
    pub signature_hash: u64,
    /// Byte span of the definition in source.
    pub span: crate::Span,
    /// True when this entry is a `<kind> clause`.
    pub is_clause: bool,
    /// For scattered union/enum clauses: the member name.
    pub member_name: Option<String>,
    /// `///` doc comment text.
    pub doc: Option<String>,
    /// Visibility of this item.
    pub visibility: crate::visibility::RawVisibility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemTree {
    /// Legacy: one entry per public top-level item, in source order.
    #[deprecated(note = "use top_level_items() with ModItem accessors")]
    pub entries: Vec<ItemTreeEntry>,
    /// Combined hash over all entries' rendered signature text.
    pub signature_hash: u64,
    /// Fixity declarations (`infixl N op`, `infixr N op`, `infix N op`).
    pub fixities: Vec<FixityDecl>,
    /// Spans of `$include` directives in this file.
    /// Each entry is (include_path_text, span_of_directive).
    /// Used by DefCollector to emit diagnostics with real source spans.
    pub include_spans: Vec<(String, crate::Span)>,

    /// Top-level items in source order, indexing into typed arenas.
    pub top_level: Vec<ModItem>,
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
pub enum ItemKind {
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
}

impl ItemTree {
    /// Top-level items in source order (typed arena version).
    pub fn top_level_items(&self) -> &[ModItem] {
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
            entries: Vec::new(),
            signature_hash: 0,
            fixities: Vec::new(),
            include_spans: Vec::new(),
            top_level: Vec::new(),
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

    /// Find a top-level item by name. O(n) — the typical use case is a
    /// single lookup per query, so a HashMap index isn't worth the cost.
    /// Returns the `ModItem` for the first match.
    /// Legacy lookup by name in `entries`.
    #[deprecated(note = "use find_by_name()")]
    #[allow(deprecated)]
    pub fn entry(&self, name: &str) -> Option<&ItemTreeEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn find_by_name(&self, name: &str) -> Option<ModItem> {
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
            let base_bp = (decl.level as u8) * 2 + 1;
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
    pub fn items_of_kind(&self, kind: ItemKind) -> impl Iterator<Item = ModItem> + '_ {
        self.top_level.iter().copied().filter(move |id| id.item_kind(self) == kind)
    }
}

/// Legacy: compute signature hash directly from typed arenas.
/// Superseded by `ItemTree::compute_per_item_hashes` .
#[allow(dead_code)]
fn compute_signature_hash(tree: &ItemTree) -> u64 {
    let mut hasher = DefaultHasher::new();
    tree.top_level.len().hash(&mut hasher);
    for id in &tree.top_level {
        id.item_kind(tree).hash(&mut hasher);
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
