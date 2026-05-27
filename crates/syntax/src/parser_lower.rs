use crate::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    TopLevel,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Value,
    Mapping,
    Overload,
    Register,
    Parameter,
    Type,
    Struct,
    Union,
    Bitfield,
    Enum,
    EnumMember,
    Newtype,
    Let,
    Var,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclRole {
    Declaration,
    Definition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    pub role: DeclRole,
    pub scope: Scope,
    /// Full span of the declaration (keyword + name + body).
    pub span: Span,
    /// Span of just the binding name (for PatId matching in type inference).
    /// Definition holds NameId, not declaration-level span.
    pub name_span: Option<Span>,
    pub is_scattered: bool,
    /// J3-4: Doc comment text (`///` comments preceding the declaration).
    ///
    /// Upstream Sail (commit f7a67174): enum members can have doc comments.
    pub doc: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedFile {
    pub decls: Vec<Decl>,
    pub type_aliases: Vec<TypeAlias>,
    pub call_sites: Vec<CallSite>,
    pub typed_bindings: Vec<TypedBinding>,
    pub callable_heads: Vec<CallableHead>,
    pub symbol_occurrences: Vec<SymbolOccurrence>,
    /// Union constructor names (e.g., `Mk_Foo` from `union Foo = { Mk_Foo : T }`).
    pub union_constructor_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAlias {
    pub sub: String,
    pub sup: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub caller: Option<String>,
    pub callee: String,
    pub callee_span: Span,
    pub open_span: Span,
    pub close_span: Option<Span>,
    pub arg_separator_spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedBinding {
    pub name: String,
    pub name_span: Span,
    pub ty_span: Span,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableParam {
    pub span: Span,
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub ty_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableHead {
    pub name: String,
    pub kind: DeclKind,
    pub name_span: Span,
    pub label_span: Span,
    pub params: Vec<CallableParam>,
    pub return_type_span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolOccurrenceKind {
    Value,
    Type,
    TypeVar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolOccurrence {
    pub name: String,
    pub kind: SymbolOccurrenceKind,
    pub span: Span,
    pub scope: Option<Scope>,
    pub role: Option<DeclRole>,
    pub target_span: Option<Span>,
}

#[allow(dead_code)]
fn push_decl(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    scope: Scope,
    span: Span,
    is_scattered: bool,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope,
        span,
        name_span: None,
        is_scattered,
        doc: None,
    });
}
