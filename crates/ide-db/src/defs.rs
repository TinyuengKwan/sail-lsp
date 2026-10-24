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
    /// Sail writes all six with one `type_def` form, so they are one arm.
    TypeDef(hir::TypeDef),
    Field(Field),
    EnumVariant(EnumVariant),
    BuiltinType(BuiltinType),
    Local(hir::Local),
    /// Sail only has type variables ('n, 'm, etc.) — no const/lifetime params.
    TypeVar(hir::TypeVar),
    Register(hir::Register),
    Mapping(hir::Mapping),
    Let(hir::DefLocation),
    Overload(hir::DefLocation),
    /// `val f : int -> int` — a signature standing on its own. Not a
    /// function: it declares the type without supplying a body, and Sail
    /// lets the two be written far apart or in different files.
    ValSpec(hir::DefLocation),
    /// The `scattered function|mapping|union|enum f` opening a scattered
    /// definition, and the `union clause` / `enum clause` adding to one.
    ScatteredHead(hir::DefLocation),
    ScatteredClause(hir::DefLocation),
    /// The `end f` closing a scattered definition.
    EndMarker(hir::DefLocation),
    /// `constraint T`.
    Constraint(hir::DefLocation),
    /// `termination_measure f = ...`.
    TerminationMeasure(hir::DefLocation),
    /// `instantiation f with 'a = ...`.
    Instantiation(hir::DefLocation),
    /// `outcome f : 'a -> unit`.
    Outcome(hir::DefLocation),
    /// A top-level name this file's resolver cannot place. See
    /// [`UnresolvedName`].
    Unresolved(UnresolvedName),
}

/// A struct or bitfield field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    pub file_id: Option<base_db::FileId>,
    pub parent_name: hir::Name,
    pub name: hir::Name,
    /// The declared type as written, when the declaration was in reach.
    pub ty_text: Option<String>,
}

/// An enum or union variant/constructor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumVariant {
    pub file_id: Option<base_db::FileId>,
    pub parent_name: hir::Name,
    pub name: hir::Name,
    /// The payload type as written, when the constructor carries one.
    pub ty_text: Option<String>,
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
    pub fn name(&self, db: &dyn salsa::Database) -> Option<hir::Name> {
        match self {
            Self::Function(f) => Some(f.name(db)),
            Self::TypeDef(t) => Some(t.name(db)),
            Self::Register(r) => Some(r.name(db)),
            Self::Mapping(m) => Some(m.name(db)),
            Self::Let(loc)
            | Self::Overload(loc)
            | Self::ValSpec(loc)
            | Self::ScatteredHead(loc)
            | Self::ScatteredClause(loc)
            | Self::EndMarker(loc)
            | Self::Constraint(loc)
            | Self::TerminationMeasure(loc)
            | Self::Instantiation(loc)
            | Self::Outcome(loc) => Some(loc.lookup_name(db)),
            Self::Field(f) => Some(f.name.clone()),
            Self::EnumVariant(v) => Some(v.name.clone()),
            Self::BuiltinType(b) => Some(hir::Name::new(b.name)),
            Self::Local(l) => Some(l.name.clone()),
            Self::TypeVar(tv) => Some(tv.name.clone()),
            Self::Unresolved(u) => Some(u.name.clone()),
        }
    }

    /// The file containing this definition.
    pub fn file_id(&self, db: &dyn salsa::Database) -> Option<base_db::FileId> {
        match self {
            Self::Function(f) => Some(f.file_id(db)),
            Self::TypeDef(t) => Some(t.file_id(db)),
            Self::Register(r) => Some(r.file_id(db)),
            Self::Mapping(m) => Some(m.file_id(db)),
            Self::Let(loc)
            | Self::Overload(loc)
            | Self::ValSpec(loc)
            | Self::ScatteredHead(loc)
            | Self::ScatteredClause(loc)
            | Self::EndMarker(loc)
            | Self::Constraint(loc)
            | Self::TerminationMeasure(loc)
            | Self::Instantiation(loc)
            | Self::Outcome(loc) => Some(loc.file_id(db)),
            Self::Field(field) => field.file_id,
            Self::EnumVariant(variant) => variant.file_id,
            Self::BuiltinType(_) | Self::Local(_) | Self::TypeVar(_) | Self::Unresolved(_) => None,
        }
    }

    /// Get the DefLocation for this definition, if it has one.
    pub fn def_location(&self) -> Option<hir::DefLocation> {
        match self {
            Self::Function(f) => Some(hir::Def::Function(*f).location()),
            Self::TypeDef(t) => Some(hir::Def::TypeDef(*t).location()),
            Self::Register(r) => Some(hir::Def::Register(*r).location()),
            Self::Mapping(m) => Some(hir::Def::Mapping(*m).location()),
            Self::Let(loc)
            | Self::Overload(loc)
            | Self::ValSpec(loc)
            | Self::ScatteredHead(loc)
            | Self::ScatteredClause(loc)
            | Self::EndMarker(loc)
            | Self::Constraint(loc)
            | Self::TerminationMeasure(loc)
            | Self::Instantiation(loc)
            | Self::Outcome(loc) => Some(*loc),
            Self::Field(_)
            | Self::EnumVariant(_)
            | Self::BuiltinType(_)
            | Self::Local(_)
            | Self::TypeVar(_)
            | Self::Unresolved(_) => None,
        }
    }

    /// Get the documentation for this definition.
    ///
    /// Returns `Documentation` wrapper.
    pub fn docs(&self, db: &dyn salsa::Database) -> Option<crate::documentation::Documentation> {
        use crate::documentation::Documentation;

        if let Self::BuiltinType(b) = self {
            return Some(Documentation::new(format!("Built-in type `{}`.", b.name)));
        }
        self.def_location()?.doc_text(db).map(Documentation::new)
    }

    /// Get the signature text for this definition.
    ///
    /// signature string for hover/completion. Looks up the signature
    /// from the ItemTree.
    pub fn signature_text(&self, db: &dyn salsa::Database) -> Option<String> {
        match self {
            Self::BuiltinType(b) => return Some(b.name.to_string()),
            Self::Field(f) => return Some(f.name.to_string()),
            Self::EnumVariant(v) => return Some(v.name.to_string()),
            Self::Local(l) => return Some(l.name.to_string()),
            Self::TypeVar(tv) => return Some(tv.name.to_string()),
            _ => {}
        }
        self.def_location()?.signature_text(db)
    }
}

impl Definition {
    /// Lift a `hir::Def` to the IDE's symbol vocabulary.
    ///
    /// `None` for a field or a union/enum member: `Definition` names those
    /// by parent and member, and a location alone does not say which type
    /// they belong to. Every other form of Sail definition has an arm.
    pub fn from_def(def: hir::Def) -> Option<Self> {
        Some(match def {
            hir::Def::Function(f) => Definition::Function(f),
            hir::Def::TypeDef(t) => Definition::TypeDef(t),
            hir::Def::Register(r) => Definition::Register(r),
            hir::Def::Mapping(m) => Definition::Mapping(m),
            hir::Def::Let(loc) => Definition::Let(loc),
            hir::Def::Overload(loc) => Definition::Overload(loc),
            hir::Def::ValSpec(loc) => Definition::ValSpec(loc),
            hir::Def::ScatteredHead(loc) => Definition::ScatteredHead(loc),
            hir::Def::ScatteredClause(loc) => Definition::ScatteredClause(loc),
            hir::Def::EndMarker(loc) => Definition::EndMarker(loc),
            hir::Def::Constraint(loc) => Definition::Constraint(loc),
            hir::Def::TerminationMeasure(loc) => Definition::TerminationMeasure(loc),
            hir::Def::Instantiation(loc) => Definition::Instantiation(loc),
            hir::Def::Outcome(loc) => Definition::Outcome(loc),
            hir::Def::Field(_) | hir::Def::Variant(_) => return None,
        })
    }
}

impl Definition {
    /// What a resolved path names, as an IDE symbol.
    pub fn from_path_resolution(res: hir::PathResolution) -> Option<Self> {
        Some(match res {
            hir::PathResolution::Def(def) => return Definition::from_def(def),
            hir::PathResolution::Local(local) => Definition::Local(local),
            hir::PathResolution::TypeVar(tv) => Definition::TypeVar(tv),
        })
    }
}

/// What the identifier at `position` is.
///
/// rust-analyzer's `ide` names `Definition` 464 times; every navigation
/// feature resolves the cursor to one and then asks *it* — `def.usages(sema)`,
/// `def.rename(...)`. Ours named it 18 times and passed a
/// `ResolvedSymbol { name: String, .. }` around instead, so `FindUsages`
/// searched by name with a byte span as a tie-breaker. Two bindings that
/// spell the same thing are one name and two definitions, and only the
/// second of those is an identity.
///
/// Local first, through the body's scope tree: a use resolves to whichever
/// binding the scope chain reaches, so a shadowed `x` and the one hiding it
/// are told apart here rather than downstream. Only when no body claims the
/// name does this fall back to what the file's resolver says.
/// Whether a binding at `binding` is one of its callable's parameters.
///
/// A parameter's `PatId` in `Body::params` is the id of the *typed*
/// pattern — `x : bits(32)` — while the scope tree binds the `IDENT_PAT`
/// inside it, so comparing ids says no for every annotated parameter.
/// Containment is the question being asked.
fn is_param(entry: &hir::CallableBody, binding: parser::Span) -> bool {
    entry.body.params.iter().any(|p| {
        entry.source_map.pat_syntax(*p).is_some_and(|param_span| {
            param_span.start <= binding.start && binding.end <= param_span.end
        })
    })
}

impl Definition {
    pub fn classify_at(
        file: &dyn crate::FileDb,
        position: crate::line_index::LineCol,
    ) -> Option<Definition> {
        match IdentClass::classify_at(file, position)? {
            IdentClass::NameClass(it) => it.defined(),
            IdentClass::NameRefClass(NameRefClass::Definition(it)) => Some(it),
        }
    }

    /// Where the binding this names is written, for the definitions that
    /// are one binding in one body. A top-level name has many possible
    /// sites — a `val` and its clauses — and answers `None`.
    pub fn binding_span(&self) -> Option<hir::Span> {
        match self {
            Definition::Local(l) => Some(l.span),
            _ => None,
        }
    }

    /// Whether this definition lives inside a body rather than at the top
    /// level, which decides whether a search may leave the file.
    pub fn is_local(&self) -> bool {
        matches!(self, Definition::Local(_) | Definition::TypeVar(_))
    }

    /// Signatures own their implementation clauses until the next signature
    /// replaces them. Keep this relation separate from declaration-site
    /// classification, which must still identify the token under the cursor.
    pub fn canonical(&self, db: &dyn salsa::Database) -> Self {
        if self
            .def_location()
            .is_none_or(|loc| hir::source_order::declarations_for(db, loc.file_text).is_none())
            || !matches!(
                self,
                Self::Function(_)
                    | Self::Mapping(_)
                    | Self::ValSpec(_)
                    | Self::ScatteredHead(_)
                    | Self::ScatteredClause(_)
                    | Self::EndMarker(_)
                    | Self::TerminationMeasure(_)
                    | Self::Outcome(_)
            )
        {
            return self.clone();
        }
        let Some(loc) = self.def_location() else { return self.clone() };
        let Some((tree, item)) = loc.item(db) else { return self.clone() };
        if tree.generated.contains_key(&item) {
            return self.clone();
        }
        let Some(at) = hir::source_order::at_offset(db, loc.file_text, item.name_span(tree).start)
        else {
            return self.clone();
        };
        let selected = if matches!(self, Self::ScatteredHead(_)) {
            hir::source_order::scattered::callable_head_signature(db, at)
        } else {
            hir::source_order::bindings::resolve(
                db,
                at,
                item.name(tree).clone(),
                hir::source_order::bindings::Namespace::Callable,
            )
        };
        selected
            .and_then(|decl| {
                hir::def_at_declaration_in_namespace(
                    db,
                    decl,
                    item.name(tree).as_str(),
                    hir::source_order::bindings::Namespace::Callable,
                )
            })
            .and_then(Self::from_def)
            .unwrap_or_else(|| self.clone())
    }
}

/// Either side of the name/name-reference split, as rust-analyzer's
/// `IdentClass` has it: a `Name` defines something, a `NameRef` refers to
/// something, and one classifier decides which so that every feature agrees.
///
/// This lived in `ide/src/goto_definition.rs` and answered in strings
/// (`Reference { name: String }`), so it could serve one feature.
#[derive(Debug, Clone)]
pub enum IdentClass {
    NameClass(NameClass),
    NameRefClass(NameRefClass),
}

impl IdentClass {
    pub fn classify_at(
        file: &dyn crate::FileDb,
        position: crate::line_index::LineCol,
    ) -> Option<IdentClass> {
        let (token, span) = file.token_at(position)?;
        let name = crate::token_symbol_key(token)?;
        let key = hir::Name::new(&name);

        if let Some((db, ft)) = file
            .file_text()
            .filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
        {
            if let Some(at) = hir::source_order::at_offset(db, ft, span.start) {
                if hir::source_order::node(db, at).is_some_and(|node| {
                    !matches!(
                        node.kind(),
                        syntax::SyntaxKind::INSTANTIATION_DEF
                            | syntax::SyntaxKind::SCATTERED_CLAUSE_DEF
                            | syntax::SyntaxKind::END_DEF
                    ) && syntax::definition_name_span(&node) == Some(*span)
                }) {
                    if let Some(def) =
                        hir::def_at_declaration(db, at, &name).and_then(Definition::from_def)
                    {
                        return Some(IdentClass::NameClass(NameClass::Definition(def)));
                    }
                }
            }
        }

        // Bitfield indices and update labels are fields even when a local
        // variable has the same spelling. Classify their receiver first.
        if let Some(field) = classify_field_access(file, &name, span) {
            return Some(IdentClass::NameRefClass(NameRefClass::Definition(Definition::Field(
                field,
            ))));
        }

        if let Some((db, ft)) = file
            .file_text()
            .filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
        {
            if syntax::parse_query::semantic_parse_file(db, ft)
                .syntax_node()
                .is_some_and(|root| syntax::utils::is_named_callee(&root, span.start))
            {
                if let Some(hir::PathResolution::Def(hir::Def::Overload(loc))) =
                    hir::Semantics::new(db).resolve_path(db, ft, span.start, &name)
                {
                    return Some(IdentClass::NameRefClass(NameRefClass::Definition(
                        Definition::Overload(loc),
                    )));
                }
            }
        }

        // A pattern naming an enum member or a union constructor lowers to
        // the same `Pat::Bind` a fresh binding does, but it is a use of a
        // top-level name, not a local.
        let names_a_member = file
            .file_text()
            .is_none_or(|(db, ft)| hir::source_order::declarations_for(db, ft).is_none())
            && file.item_tree().is_some_and(|tree| {
                tree.top_level_items().iter().any(|id| {
                    matches!(id.def_kind(tree), hir::DefKind::Enum | hir::DefKind::Union)
                        && id.fields(tree).iter().any(|f| f.name.as_str() == name)
                })
            })
            || classify_enum_member(file, &name, span.start).is_some();
        if let Some(bodies) = file.bodies().filter(|_| !names_a_member) {
            for entry in bodies.entries() {
                if span.start < entry.def_span.start || span.end > entry.def_span.end {
                    continue;
                }
                let scopes = file.body_scopes(entry);
                // The cursor on a use: resolve it through the scope chain.
                let at_use = entry.body.iter_exprs().find_map(|(expr_id, expr)| {
                    let hir::Expr::Ident(n) = expr else { return None };
                    if n.as_str() != name || entry.source_map.expr_syntax(expr_id) != Some(*span) {
                        return None;
                    }
                    let pat = scopes
                        .scope_for(expr_id)
                        .and_then(|scope| scopes.resolve_name_in_scope(scope, &key))?
                        .pat();
                    let binding = entry.source_map.pat_syntax(pat)?;
                    Some((binding, is_param(entry, binding)))
                });
                if let Some((binding, is_param)) = at_use {
                    let local =
                        Definition::Local(hir::Local { name: key, span: binding, is_param });
                    return Some(if binding == *span {
                        IdentClass::NameClass(NameClass::Definition(local))
                    } else {
                        IdentClass::NameRefClass(NameRefClass::Definition(local))
                    });
                }
                // Or on the binding itself.
                let at_binding = entry.body.iter_pats().any(|(pat_id, pat)| {
                    matches!(pat, hir::Pat::Bind(n) | hir::Pat::TypeVar(n) if n.as_str() == name)
                        && scopes.binds_pattern(pat_id)
                        && entry.source_map.pat_syntax(pat_id) == Some(*span)
                });
                if at_binding {
                    return Some(IdentClass::NameClass(NameClass::Definition(Definition::Local(
                        hir::Local { name: key, span: *span, is_param: is_param(entry, *span) },
                    ))));
                }
            }
        }

        // A field name inside a struct literal or a struct pattern. It is
        // not a local and the resolver has no top-level name for it, so it
        // came back `Unresolved` — hover said the bare word and
        // goto-definition had nowhere to go, for every `struct { variety =
        // .. }` in the corpus. `Definition::Field` was classified nowhere
        // and dispatched nowhere; this is what builds one.
        if let Some(field) = classify_struct_field(file, &name, span) {
            return Some(IdentClass::NameRefClass(NameRefClass::Definition(Definition::Field(
                field,
            ))));
        }

        // A constructor: an enum member, a union constructor, or the one a
        // `newtype` declares. The *type* carries the name, not the item, so
        // the resolver has nothing to answer with and every use came back
        // `Unresolved` — including `Regno` and `Virtaddr`, which sail-riscv
        // writes on every address and register number.
        if let Some(member) = classify_enum_member(file, &name, span.start) {
            return Some(IdentClass::NameRefClass(NameRefClass::Definition(
                Definition::EnumVariant(member),
            )));
        }

        // Not a local. Ask the file's resolver what the name is; when it
        // cannot say — the definition is in a file this handle does not
        // reach — the name still denotes something at the top level, and
        // the workspace search is what finds it.
        let resolved = file
            .file_text()
            .and_then(|(db, ft)| hir::Semantics::new(db).resolve_path(db, ft, span.start, &name))
            .and_then(Definition::from_path_resolution);
        let def = match resolved {
            Some(def) => def,
            None if name.starts_with('\'') => Definition::TypeVar(hir::TypeVar { name: key }),
            None if file.file_text().is_some_and(|(db, ft)| {
                base_db::module_graph::file_scope(db, ft.file_id(db)).is_some()
                    || hir::source_order::declarations_for(db, ft).is_some()
            }) =>
            {
                return None
            }
            None => Definition::Unresolved(UnresolvedName { name: key }),
        };
        // Whether the cursor sits on the declaration or on a use of it is a
        // question about this token, not about the definition.
        let declares = file.item_tree().is_some_and(|tree| {
            tree.top_level_items().iter().any(|id| {
                !matches!(
                    id.def_kind(tree),
                    hir::DefKind::Instantiation
                        | hir::DefKind::ScatteredClause
                        | hir::DefKind::EndMarker
                ) && id.name(tree).as_str() == name
                    && id.name_span(tree) == *span
            })
        });
        Some(if declares {
            IdentClass::NameClass(NameClass::Definition(def))
        } else {
            IdentClass::NameRefClass(NameRefClass::Definition(def))
        })
    }
}

/// A top-level name whose definition this file's resolver cannot see.
///
/// rust-analyzer has no such arm: its database holds every crate, so a
/// name either resolves or is an error. Ours reaches the workspace through
/// a symbol index rather than through name resolution, so a name declared
/// in another file arrives here unresolved and is still a real definition
/// to search for. Deleting this arm means teaching the resolver to cross
/// files, not dropping the case.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnresolvedName {
    pub name: hir::Name,
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

use hir::display::{HirDisplay, HirDisplayError, HirFormatter};
use std::fmt::Write as _;

/// What a definition reads as, in Sail's own words.
///
/// rust-analyzer's `ide` renders a definition through `HirDisplay`; here
/// nothing did, so the impls in `hir/src/display.rs` — one per definition
/// form — had no caller at all, and the formatter they render into had an
/// optional database that was always `None`.
impl HirDisplay for Definition {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        match self {
            Definition::Function(it) => it.hir_fmt(f),
            Definition::TypeDef(it) => it.hir_fmt(f),
            Definition::Register(it) => it.hir_fmt(f),
            Definition::Mapping(it) => it.hir_fmt(f),
            Definition::Field(it) => it.hir_fmt(f),
            Definition::EnumVariant(it) => it.hir_fmt(f),
            Definition::Local(it) => it.hir_fmt(f),
            Definition::TypeVar(it) => it.hir_fmt(f),
            Definition::BuiltinType(it) => {
                write!(f, "{}", it.name)?;
                Ok(())
            }
            // The rest are a keyword and a name at a location.
            Definition::Let(loc) => keyword_and_name(f, "let", loc),
            Definition::Overload(loc) => keyword_and_name(f, "overload", loc),
            Definition::ValSpec(loc) => keyword_and_name(f, "val", loc),
            Definition::ScatteredHead(loc) => keyword_and_name(f, "scattered", loc),
            Definition::ScatteredClause(loc) => keyword_and_name(f, "clause", loc),
            Definition::EndMarker(loc) => keyword_and_name(f, "end", loc),
            Definition::Constraint(loc) => keyword_and_name(f, "constraint", loc),
            Definition::TerminationMeasure(loc) => keyword_and_name(f, "termination_measure", loc),
            Definition::Instantiation(loc) => keyword_and_name(f, "instantiation", loc),
            Definition::Outcome(loc) => keyword_and_name(f, "outcome", loc),
            // A name this file's resolver could not place has nothing to
            // render but itself.
            Definition::Unresolved(it) => {
                write!(f, "{}", it.name)?;
                Ok(())
            }
        }
    }
}

/// `keyword name`, with the signature when the item has one.
///
/// A `val` without its signature says less than the source line it names,
/// which is why hover fell back to the scrape for these.
fn keyword_and_name(
    f: &mut HirFormatter<'_>,
    keyword: &str,
    loc: &hir::DefLocation,
) -> Result<(), HirDisplayError> {
    let name = loc.lookup_name(f.db);
    match loc.lookup_signature(f.db) {
        Some(sig) => write!(f, "{keyword} {name} : {sig}")?,
        None => write!(f, "{keyword} {name}")?,
    }
    Ok(())
}

/// Renders as `parent.field_name : T`.
impl HirDisplay for Field {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "{}.{}", self.parent_name.as_str(), self.name.as_str())?;
        if let Some(ty) = self.ty_text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            write!(f, " : {ty}")?;
        }
        Ok(())
    }
}

/// Renders as `Parent::Variant`, with the payload type when there is one.
impl HirDisplay for EnumVariant {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result<(), HirDisplayError> {
        write!(f, "{}::{}", self.parent_name.as_str(), self.name.as_str())?;
        if let Some(ty) = self.ty_text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            write!(f, " : {ty}")?;
        }
        Ok(())
    }
}

/// The enum member, union constructor or `newtype` constructor a token
/// names.
///
/// Exactly one declaration must have a member by that name; two say
/// nothing rather than guessing, the same rule the field classifier
/// follows.
fn classify_enum_member(
    file: &dyn crate::FileDb,
    name: &str,
    offset: usize,
) -> Option<EnumVariant> {
    fn member(
        tree: &hir::ItemTree,
        name: &str,
        file_id: Option<base_db::FileId>,
        declaration_range: Option<base_db::TextRange>,
    ) -> Option<EnumVariant> {
        let mut found = None;
        for &id in tree.top_level_items() {
            if declaration_range
                .is_some_and(|r| !r.contains((id.name_span(tree).start as u32).into()))
            {
                continue;
            }
            if matches!(
                id.def_kind(tree),
                hir::DefKind::Enum | hir::DefKind::Union | hir::DefKind::Newtype
            ) {
                if let Some(field) = id.fields(tree).iter().find(|f| f.name.as_str() == name) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(EnumVariant {
                        file_id,
                        parent_name: id.name(tree).clone(),
                        name: hir::Name::new(name),
                        ty_text: (!field.type_text.is_empty()).then(|| field.type_text.clone()),
                    });
                }
            }
            if id.scattered_member(tree).is_some_and(|(n, _)| n == name) {
                if found.is_some() {
                    return None;
                }
                found = Some(EnumVariant {
                    file_id,
                    parent_name: id.name(tree).clone(),
                    name: hir::Name::new(name),
                    ty_text: None,
                });
            }
        }
        found
    }
    if let Some((db, ft)) =
        file.file_text().filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
    {
        use hir::source_order::{
            self,
            bindings::{self, Namespace},
        };
        let at = source_order::at_offset(db, ft, offset)?;
        let node = source_order::node(db, at)?;
        if node
            .token_at_offset((offset as u32).into())
            .right_biased()
            .and_then(|t| t.parent())
            .is_some_and(|p| p.kind().is_type_node())
        {
            return None;
        }
        // Member declaration tokens name themselves; references in payload
        // types still resolve in the declaration's preceding environment.
        let tree = hir::def_query::file_item_tree(db, ft).as_ref()?;
        let declares = tree.top_level_items().iter().any(|item| {
            item.fields(tree).iter().any(|f| f.name.as_str() == name && f.name_span.start == offset)
                || item
                    .scattered_member(tree)
                    .is_some_and(|(n, span)| n == name && span.start == offset)
        });
        let selected = if declares {
            at
        } else {
            bindings::resolve(db, at, hir::Name::new(name), Namespace::Value)?
        };
        let range = if selected == at {
            node.text_range()
        } else {
            source_order::node(db, selected)?.text_range()
        };
        return member(
            hir::def_query::file_item_tree(db, selected.file).as_ref()?,
            name,
            Some(selected.file.file_id(db)),
            Some(range),
        );
    }
    let origin = file.file_text().map(|(db, ft)| ft.file_id(db));
    if let Some(found) = file.item_tree().and_then(|tree| member(tree, name, origin, None)) {
        return Some(found);
    }
    let (db, from) = file.file_text()?;
    base_db::module_graph::file_scope(db, from.file_id(db))?;
    let ws = base_db::WorkspaceFiles::try_get(db)?;
    for &target in
        hir::visibility::visible_files_for_name(db, ws, from.file_id(db), hir::Name::new(name))
    {
        if let Some(tree) = hir::def_query::file_item_tree(db, target) {
            if let Some(found) = member(tree, name, Some(target.file_id(db)), None) {
                return Some(found);
            }
        }
    }
    None
}

/// The struct field a token names, when it sits in a struct literal's or a
/// struct pattern's field list.
///
/// The literal's own name wins (`struct S { .. }`); otherwise the
/// annotation around it says which struct this is —
/// `helpers::struct_literal_type_name`, the same question the
/// unknown-field diagnostic asks of the same tree.
fn classify_struct_field(file: &dyn crate::FileDb, name: &str, span: &hir::Span) -> Option<Field> {
    use parser::SyntaxKind as SK;

    let text = file.text();
    let (root, _) = syntax::parse_text(text);
    let token = root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| usize::from(t.text_range().start()) == span.start && t.text() == name)?;
    let init = token.parent().filter(|p| p.kind() == SK::FIELD_INIT)?;
    // The name, not the value: `struct { a = b }` has `b` under the same
    // `FIELD_INIT`, and it is an expression that resolves on its own.
    let is_the_name = init
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .is_some_and(|t| t.text_range() == token.text_range());
    if !is_the_name {
        return None;
    }
    let lit = init.parent().filter(|p| matches!(p.kind(), SK::STRUCT_EXPR | SK::STRUCT_PAT))?;

    let name_of = |n: &syntax::SyntaxNode| {
        n.children()
            .find(|c| c.kind() == SK::NAME)
            .and_then(|n| {
                n.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
    };
    let own_name = name_of(&lit);
    // The *declaration* is the same two nodes. `struct st = { One : int }`
    // puts its field list in a `STRUCT_EXPR` with one `FIELD_INIT` per field,
    // exactly as a literal does, and the only difference is where the type's
    // name sits: on the enclosing definition rather than on the list.
    //
    // Reading only the list meant a field name at the place it is *declared*
    // classified as nothing, so hover said nothing there — while the item
    // tree had already recorded the field, its span and its declared type.
    // On sail-riscv this was the largest single group of hover misses.
    //
    // The enclosing keyword has to be checked: `let x = struct { .. }` is a
    // `NAMED_DEF` too, and its name is the binding's, not a struct's.
    let declared_in = || {
        let def = lit.parent()?;
        let declares = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia())
            .is_some_and(|t| {
                matches!(t.kind(), SK::KW_STRUCT | SK::KW_UNION | SK::KW_ENUM | SK::KW_BITFIELD)
            });
        declares.then(|| name_of(&def)).flatten()
    };
    let parent_name =
        own_name.or_else(declared_in).or_else(|| crate::helpers::struct_literal_type_name(&lit));

    let tree = file.item_tree()?;
    let declares_fields = |id: &hir::DefItem| {
        matches!(
            id.def_kind(tree),
            hir::DefKind::Struct | hir::DefKind::Bitfield | hir::DefKind::Union
        )
    };
    let as_field = |parent: &str, decl: &hir::FieldDecl, file_id| Field {
        file_id,
        parent_name: hir::Name::new(parent),
        name: hir::Name::new(name),
        ty_text: (!decl.type_text.is_empty()).then(|| decl.type_text.clone()),
    };
    // A record is as often declared in another file as in this one:
    // sail-riscv writes `Mem_write_request` and `Explicit_access_kind`
    // literals in `phys_mem_interface.sail` while the structs are in the
    // bundled concurrency interface. Whichever way the record's *name* was
    // reached — written on an annotation, or inferred — the declaration was
    // then looked for in this file alone, and a name with no declaration
    // beside it classified as nothing.
    let named = |record: &str| -> Option<Field> {
        if let Some((db, ft)) = file
            .file_text()
            .filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
        {
            return ordered_field(db, ft, span.start, record, name);
        }
        if let Some(id) = tree
            .top_level_items()
            .iter()
            .find(|id| id.name(tree).as_str() == record && declares_fields(id))
        {
            if let Some(decl) = id.fields(tree).iter().find(|f| f.name.as_str() == name) {
                return Some(as_field(
                    record,
                    decl,
                    file.file_text().map(|(db, ft)| ft.file_id(db)),
                ));
            }
        }
        let (db, ft) = file.file_text()?;
        let declared_in = hir::Semantics::new(db).declaring_type_file(ft, record)?;
        let other = hir::def_query::file_item_tree(db, declared_in).as_deref()?;
        let id = other.top_level_items().iter().find(|id| {
            id.name(other).as_str() == record
                && matches!(
                    id.def_kind(other),
                    hir::DefKind::Struct | hir::DefKind::Bitfield | hir::DefKind::Union
                )
        })?;
        let decl = id.fields(other).iter().find(|f| f.name.as_str() == name)?;
        Some(as_field(record, decl, Some(declared_in.file_id(db))))
    };

    if let Some(ref parent_name) = parent_name {
        return named(parent_name);
    }

    // No annotation to read: a literal passed straight to a call
    // (`AK_explicit(struct { variety = .. })`) takes its type from the
    // parameter, and `[o with LCOFI = ..]` from the base. Neither is on the
    // tree beside the literal.
    //
    // Ask inference first — that is what the expectation carried into the
    // literal, and the reason its field values get checked at all. 76 of
    // hover's 123 misses on sail-riscv were this shape: the scrape
    // re-deriving what the lowering already computed.
    let inferred = file.file_text().and_then(|(db, ft)| {
        hir::Semantics::new(db).type_of_expr(ft, usize::from(lit.text_range().start()))
    });
    if let Some(field) = inferred.as_ref().and_then(|ty| ty.as_name()).and_then(|r| named(&r)) {
        return Some(field);
    }

    if file
        .file_text()
        .is_some_and(|(db, ft)| hir::source_order::declarations_for(db, ft).is_some())
    {
        return None;
    }

    // Failing that, the field name itself decides it when exactly *one*
    // declaration in this file has a field by that name. Ambiguity says
    // nothing — which is what it said before — so this can be silent but
    // never wrong.
    let mut matches = tree
        .top_level_items()
        .iter()
        .filter(|id| declares_fields(id))
        .filter(|id| id.fields(tree).iter().any(|f| f.name.as_str() == name));
    let only = *matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let decl = only.fields(tree).iter().find(|f| f.name.as_str() == name)?;
    Some(as_field(only.name(tree).as_str(), decl, file.file_text().map(|(db, ft)| ft.file_id(db))))
}

fn ordered_field(
    db: &dyn salsa::Database,
    file: base_db::FileText,
    offset: usize,
    record: &str,
    name: &str,
) -> Option<Field> {
    use hir::source_order::{
        self,
        bindings::{self, Namespace},
    };
    let at = source_order::at_offset(db, file, offset)?;
    let node = source_order::node(db, at)?;
    let own = syntax::definition_name(&node).as_deref() == Some(record)
        && node.kind() == parser::SyntaxKind::NAMED_DEF;
    let selected =
        if own { at } else { bindings::resolve(db, at, hir::Name::new(record), Namespace::Type)? };
    let def = hir::def_at_declaration_in_namespace(db, selected, record, Namespace::Type)?;
    let loc = def.location();
    let (tree, item) = loc.item(db)?;
    let field = item.fields(tree).iter().find(|f| f.name.as_str() == name)?;
    Some(Field {
        file_id: Some(selected.file.file_id(db)),
        parent_name: hir::Name::new(record),
        name: field.name.clone(),
        ty_text: (!field.type_text.is_empty()).then(|| field.type_text.clone()),
    })
}

fn classify_field_access(file: &dyn crate::FileDb, name: &str, span: &hir::Span) -> Option<Field> {
    let (db, ft) = file.file_text()?;
    if hir::source_order::declarations_for(db, ft).is_none() {
        return None;
    }
    let root = syntax::parse_query::semantic_parse_file(db, ft).syntax_node()?;
    let token = root.token_at_offset((span.start as u32).into()).right_biased()?;
    if !token.parent_ancestors().any(|node| {
        matches!(
            node.kind(),
            syntax::SyntaxKind::FIELD_ACCESS_EXPR
                | syntax::SyntaxKind::INDEX_EXPR
                | syntax::SyntaxKind::VECTOR_UPDATE_EXPR
        )
    }) {
        return None;
    }
    for entry in file.bodies()?.entries() {
        if span.start < entry.def_span.start || span.end > entry.def_span.end {
            continue;
        }
        for (id, expr) in entry.body.iter_exprs() {
            let (receiver, index) = match expr {
                hir::Expr::Field { expr, field } if field == name => (*expr, None),
                hir::Expr::Index { base, index } => (*base, Some(*index)),
                hir::Expr::Call { callee, args }
                    if matches!(entry.body.expr(*callee), Some(hir::Expr::Ident(name)) if name == "vector_update#")
                        && args.len() == 3 =>
                {
                    (args[0], Some(args[1]))
                }
                _ => continue,
            };
            if let Some(index) = index {
                if !matches!(entry.body.expr(index), Some(hir::Expr::Ident(field)) if field == name)
                    || entry.source_map.expr_syntax(index) != Some(*span)
                {
                    continue;
                }
            }
            let Some(full) = entry.source_map.expr_syntax(id) else { continue };
            let Some(base) = entry.source_map.expr_syntax(receiver) else { continue };
            if index.is_none() && (span.start < base.end || span.end > full.end) {
                continue;
            }
            let receiver_type = hir::Semantics::new(db).type_of_expr(ft, base.start)?;
            let at = hir::source_order::at_offset(db, ft, span.start)?;
            let receiver_type = hir::infer::normalize_type_at(db, at, &receiver_type);
            let receiver_type = match receiver_type.kind() {
                hir::TyKind::Named(name, args) | hir::TyKind::App { name, args, .. }
                    if name == "register" && index.is_none() =>
                {
                    match args.first() {
                        Some(hir::ty::TyArg::Type(inner)) => {
                            hir::infer::normalize_type_at(db, at, inner)
                        }
                        _ => receiver_type,
                    }
                }
                _ => receiver_type,
            };
            let record = receiver_type.as_name()?;
            if index.is_some() {
                use hir::source_order::{
                    self,
                    bindings::{self, Namespace},
                };
                let at = source_order::at_offset(db, ft, span.start)?;
                let selected = bindings::resolve(db, at, hir::Name::new(record), Namespace::Type)?;
                let def =
                    hir::def_at_declaration_in_namespace(db, selected, record, Namespace::Type)?;
                let (tree, item) = def.location().item(db)?;
                if item.def_kind(tree) != hir::DefKind::Bitfield {
                    continue;
                }
            }
            return ordered_field(db, ft, span.start, record, name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestFile;
    use crate::FileDb;

    /// A parameter is a parameter, and a `let` is not.
    ///
    /// `Local::is_param` is rust-analyzer's, and the naive form of this
    /// check — `Body::params.contains(&pat)` — says no for every
    /// *annotated* parameter: the id in `params` is the `x : bits(32)`
    /// pattern and the scope tree binds the `IDENT_PAT` inside it.
    #[test]
    fn an_annotated_parameter_is_still_a_parameter() {
        let src = "function f(x : bits(32)) = {\n  let y = x;\n  y\n}\n";
        let file = TestFile::new(src);

        let at = |needle: &str, nth: usize| {
            let off = src.match_indices(needle).nth(nth).expect("the occurrence").0;
            Definition::classify_at(&file, file.position_at(off))
        };

        match at("x", 1) {
            Some(Definition::Local(local)) => {
                assert!(local.is_param, "an annotated parameter reported as a let binding")
            }
            other => panic!("expected a local, got {other:?}"),
        }
        match at("y", 0) {
            Some(Definition::Local(local)) => {
                assert!(!local.is_param, "a let binding reported as a parameter")
            }
            other => panic!("expected a local, got {other:?}"),
        }
    }

    /// A top-level name classifies, and renders what it declares.
    ///
    /// It answered `Unresolved` — for every top-level name, including one
    /// defined in the same file — because `Semantics::analyze` gave up
    /// when no *body* contained the offset, and a `val`'s own name sits
    /// outside every body. So the definitions that have signatures never
    /// reached the HIR at all, and `ide/src/hover` could render its header
    /// through `HirDisplay` but not its signature line.
    #[test]
    fn a_top_level_name_classifies_and_renders_its_signature() {
        use hir::HirDisplay;
        let src = "val add : int -> int\nfunction add(x) = x\n";
        let file = TestFile::new(src);
        let off = src.find("add").expect("the val's name");
        let def = Definition::classify_at(&file, file.position_at(off))
            .expect("the cursor did not classify");
        assert!(
            !matches!(def, Definition::Unresolved(_)),
            "a top-level name defined in this very file did not resolve"
        );
        let (db, _) = file.file_text().expect("a database");
        let rendered = def.display(db).to_string();
        assert!(
            rendered.contains("int -> int"),
            "a declaration rendered without what it declares: {rendered}"
        );
    }

    /// A field classifies where it is *declared*, not only where it is used.
    ///
    /// The declaration and the literal are the same two nodes — a
    /// `STRUCT_EXPR` holding one `FIELD_INIT` per field — and only the
    /// literal's name is on the list; a declaration's is on the definition
    /// above it. Reading only the list meant a field name at its declaration
    /// classified as nothing, so hover said nothing there while the item tree
    /// held the field's span and its declared type all along. It was the
    /// largest single group of hover misses on sail-riscv, 57 of 123.
    #[test]
    fn a_field_classifies_where_it_is_declared() {
        let src = "struct st = { one : int, two : bool }\n\
                   union u = { A : int }\n\
                   let x = struct { one = 1 }\n";
        let file = TestFile::new(src);
        let at = |needle: &str| {
            let off = src.find(needle).expect("the occurrence");
            Definition::classify_at(&file, file.position_at(off))
        };

        match at("one : int") {
            Some(Definition::Field(f)) => {
                assert_eq!(f.parent_name.as_str(), "st");
                assert_eq!(f.ty_text.as_deref(), Some("int"));
            }
            other => panic!("a declared struct field did not classify: {other:?}"),
        }
        match at("A : int") {
            Some(Definition::Field(f)) => assert_eq!(f.parent_name.as_str(), "u"),
            other => panic!("a declared union member did not classify: {other:?}"),
        }
        // The enclosing keyword decides it: `let x = struct { .. }` is a
        // `NAMED_DEF` too, and `x` is the binding's name, not a struct's.
        match at("one = 1") {
            Some(Definition::Field(f)) => assert_eq!(
                f.parent_name.as_str(),
                "st",
                "a literal's field took the enclosing let's name"
            ),
            other => panic!("a struct literal's field did not classify: {other:?}"),
        }
    }
}
