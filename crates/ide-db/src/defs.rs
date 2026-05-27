//! Core definition types shared across IDE crates.
//! In RA, `Definition` is the IDE-level union of all "things that can be
//! named" — module defs, locals, type params, labels, etc.  It lives in
//! `ide-db` (not `hir`) because it's an IDE concept.

/// Symbol kind — classifies top-level and local definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Variable,
    Constant,
    TypeParameter,
    Struct,
    Enum,
    EnumMember,
    Module,
    Field,
    Property,
    Event,
    Operator,
    TypeAlias,
    Other,
}

/// Completion item kind — classifies completion candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionItemKind {
    Function,
    Variable,
    Keyword,
    Snippet,
    Field,
    EnumMember,
    TypeParameter,
    Struct,
    Enum,
    Module,
    Constant,
    Operator,
    Property,
    Text,
}

/// Any named entity in the program — the IDE-level "symbol".
///
/// Sail subset: no macros, traits, generics, labels, const, static.
/// Extended with Sail-specific: Register, Mapping, Overload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Definition {
    Function(hir::Function),
    /// into a single Adt. RA has separate `Adt` + `TypeAlias` variants.
    TypeDef(hir::Adt),
    Field(Field),
    EnumVariant(EnumVariant),
    BuiltinType(BuiltinType),
    Local(hir::Local),
    /// Sail only has type variables ('n, 'm, etc.) — no const/lifetime params.
    GenericParam(hir::GenericParam),
    Register(hir::Register),
    Mapping(hir::Mapping),
    Let(hir::DefLocation),
    Overload(hir::DefLocation),
}

/// A struct or bitfield field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    pub parent_name: hir_def::Name,
    pub name: hir_def::Name,
}

/// An enum or union variant/constructor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumVariant {
    pub parent_name: hir_def::Name,
    pub name: hir_def::Name,
}

/// A built-in type (int, bool, bits, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuiltinType {
    pub name: &'static str,
}

impl BuiltinType {
    /// Check if a name is a Sail built-in type.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "int" | "nat" | "bool" | "unit" | "string" | "real" | "bit" | "bits" | "vector"
            | "list" | "option" | "result" | "range" | "atom" | "atom_bool" | "implicit" => {
                Some(Self { name: Self::intern(name) })
            }
            _ => None,
        }
    }

    fn intern(s: &str) -> &'static str {
        match s {
            "int" => "int",
            "nat" => "nat",
            "bool" => "bool",
            "unit" => "unit",
            "string" => "string",
            "real" => "real",
            "bit" => "bit",
            "bits" => "bits",
            "vector" => "vector",
            "list" => "list",
            "option" => "option",
            "result" => "result",
            "range" => "range",
            "atom" => "atom",
            "atom_bool" => "atom_bool",
            "implicit" => "implicit",
            _ => "unknown",
        }
    }
}

impl Definition {
    /// Get the name of this definition.
    pub fn name(&self, db: &dyn hir_def::db::DefDatabase) -> Option<hir_def::Name> {
        match self {
            Self::Function(f) => Some(f.name(db)),
            Self::TypeDef(t) => Some(t.name(db)),
            Self::Register(r) => Some(r.name(db)),
            Self::Mapping(m) => Some(m.name(db)),
            Self::Let(loc) | Self::Overload(loc) => Some(loc.lookup_name(db)),
            Self::Field(f) => Some(f.name.clone()),
            Self::EnumVariant(v) => Some(v.name.clone()),
            Self::BuiltinType(b) => Some(hir_def::Name::new(b.name)),
            Self::Local(l) => Some(l.name.clone()),
            Self::GenericParam(tv) => Some(tv.name.clone()),
        }
    }

    /// The file containing this definition.
    pub fn file_id(&self, db: &dyn hir_def::db::DefDatabase) -> Option<base_db::FileId> {
        match self {
            Self::Function(f) => Some(f.file_id(db)),
            Self::TypeDef(t) => Some(t.file_id(db)),
            Self::Register(r) => Some(r.file_id(db)),
            Self::Mapping(m) => Some(m.file_id(db)),
            Self::Let(loc) | Self::Overload(loc) => Some(loc.file_id(db)),
            Self::Field(_)
            | Self::EnumVariant(_)
            | Self::BuiltinType(_)
            | Self::Local(_)
            | Self::GenericParam(_) => None,
        }
    }

    /// Get the DefLocation for this definition, if it has one.
    pub(crate) fn def_location(&self) -> Option<hir::DefLocation> {
        match self {
            Self::Function(f) => Some(hir::ModuleDef::Function(*f).location()),
            Self::TypeDef(t) => Some(hir::ModuleDef::TypeDef(*t).location()),
            Self::Register(r) => Some(hir::ModuleDef::Register(*r).location()),
            Self::Mapping(m) => Some(hir::ModuleDef::Mapping(*m).location()),
            Self::Let(loc) | Self::Overload(loc) => Some(*loc),
            Self::Field(_)
            | Self::EnumVariant(_)
            | Self::BuiltinType(_)
            | Self::Local(_)
            | Self::GenericParam(_) => None,
        }
    }

    /// Get the documentation for this definition.
    ///
    /// Returns `Documentation` wrapper.
    pub fn docs(
        &self,
        db: &dyn hir_def::db::DefDatabase,
    ) -> Option<crate::documentation::Documentation> {
        use crate::documentation::Documentation;

        if let Self::BuiltinType(b) = self {
            return Some(Documentation::new(format!("Built-in type `{}`.", b.name)));
        }
        let loc = self.def_location()?;
        let item_tree = hir_def::def_query::file_item_tree(db, loc.file_text).as_ref()?;
        let def_map = hir_def::def_query::crate_def_map(db, loc.file_text).as_ref()?;
        let raw_id = loc.raw_def_id();
        let def_data = def_map.0.get(raw_id)?;
        let mod_item = item_tree.top_level_items().get(def_data.item_tree_index)?;
        mod_item.doc(item_tree).map(|s| Documentation::new(s.to_string()))
    }

    /// Get the signature text for this definition.
    ///
    /// signature string for hover/completion. Looks up the signature
    /// from the ItemTree.
    pub fn signature_text(&self, db: &dyn hir_def::db::DefDatabase) -> Option<String> {
        match self {
            Self::BuiltinType(b) => return Some(b.name.to_string()),
            Self::Field(f) => return Some(f.name.to_string()),
            Self::EnumVariant(v) => return Some(v.name.to_string()),
            Self::Local(l) => return Some(l.name.to_string()),
            Self::GenericParam(tv) => return Some(tv.name.to_string()),
            _ => {}
        }
        let loc = self.def_location()?;
        let item_tree = hir_def::def_query::file_item_tree(db, loc.file_text).as_ref()?;
        let def_map = hir_def::def_query::crate_def_map(db, loc.file_text).as_ref()?;
        let raw_id = loc.raw_def_id();
        let def_data = def_map.0.get(raw_id)?;
        let mod_item = item_tree.top_level_items().get(def_data.item_tree_index)?;
        Some(mod_item.signature(item_tree).to_string())
    }
}

/// Convert from hir::ModuleDef (compiler level) to ide-db Definition.
impl From<hir::ModuleDef> for Definition {
    fn from(def: hir::ModuleDef) -> Self {
        match def {
            hir::ModuleDef::Function(f) => Definition::Function(f),
            hir::ModuleDef::TypeDef(t) => Definition::TypeDef(t),
            hir::ModuleDef::Register(r) => Definition::Register(r),
            hir::ModuleDef::Mapping(m) => Definition::Mapping(m),
            hir::ModuleDef::Let(loc) => Definition::Let(loc),
            hir::ModuleDef::Overload(loc) => Definition::Overload(loc),
        }
    }
}

/// Convert from hir::PathResolution to ide-db Definition.
impl From<hir::PathResolution> for Definition {
    fn from(res: hir::PathResolution) -> Self {
        match res {
            hir::PathResolution::Def(def) => def.into(),
            hir::PathResolution::Local(local) => Definition::Local(local),
            hir::PathResolution::TypeParam(tv) => Definition::GenericParam(tv),
        }
    }
}

/// Classification of a name (definition site).
/// ```text
/// pub enum NameClass<'db> {
///     Definition(Definition),
///     ConstReference(Definition),
///     PatFieldShorthand { local_def, field_ref, adt_subst },
/// }
/// ```
#[derive(Debug, Clone)]
pub enum NameClass {
    /// This name defines the given `Definition`.
    Definition(Definition),
    /// This name looks like a definition syntactically, but semantically
    /// it's a reference to a constant (e.g., enum variant in pattern).
    ConstReference(Definition),
}

impl NameClass {
    /// The `Definition` defined by this name, if any.
    pub fn defined(self) -> Option<Definition> {
        match self {
            NameClass::Definition(it) => Some(it),
            NameClass::ConstReference(_) => None,
        }
    }
}

/// Classification of a name reference (usage site).
/// ```text
/// pub enum NameRefClass<'db> {
///     Definition(Definition, Option<GenericSubstitution<'db>>),
///     FieldShorthand { local_ref, field_ref, adt_subst },
///     ExternCrateShorthand { decl, krate },
/// }
/// ```
#[derive(Debug, Clone)]
pub enum NameRefClass {
    /// This name reference resolves to the given `Definition`.
    Definition(Definition),
}

use hir_ty::display::{HirDisplay, HirDisplayError, HirFormatter};
use std::fmt::Write as _;

/// Renders as `parent.field_name`.
impl HirDisplay for Field {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "{}.{}", self.parent_name.as_str(), self.name.as_str())?;
        Ok(())
    }
}

/// Renders as `Parent::Variant`.
impl HirDisplay for EnumVariant {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "{}::{}", self.parent_name.as_str(), self.name.as_str())?;
        Ok(())
    }
}
