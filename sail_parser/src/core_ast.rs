use crate::ast as parse_ast;
use crate::Span;

pub type Spanned<T> = (T, Span);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFile {
    pub defs: Vec<Spanned<Definition>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatteredKind {
    Function,
    Mapping,
    Union,
    Enum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatteredClauseKind {
    Enum,
    Union,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallableSpecKind {
    Value,
    Mapping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternBinding {
    pub name: Option<Spanned<String>>,
    pub value: Spanned<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternPurity {
    Pure,
    Impure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternSpec {
    String {
        purity: Option<ExternPurity>,
        value: Spanned<String>,
    },
    Bindings {
        purity: Option<ExternPurity>,
        bindings: Vec<Spanned<ExternBinding>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: Spanned<String>,
    pub data: Option<Spanned<String>>,
    pub parsed_data: Option<Spanned<AttributeData>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefModifiers {
    pub is_private: bool,
    pub attrs: Vec<Spanned<Attribute>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallableDefKind {
    Function,
    FunctionClause,
    Mapping,
    MappingClause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantifierVar {
    pub name: Spanned<String>,
    pub kind: Option<Spanned<String>>,
    pub is_constant: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableQuantifier {
    pub vars: Vec<QuantifierVar>,
    pub constraint: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecMeasure {
    pub pattern: Spanned<Pattern>,
    pub body: Spanned<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableClause {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
    pub patterns: Vec<Spanned<Pattern>>,
    pub guard: Option<Spanned<Expr>>,
    pub quantifier: Option<CallableQuantifier>,
    pub return_ty: Option<Spanned<TypeExpr>>,
    pub body: Option<Spanned<Expr>>,
    pub mapping_body: Option<MappingBody>,
    pub body_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingBody {
    pub arms: Vec<Spanned<MappingArm>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingArmDirection {
    Bidirectional,
    Forwards,
    Backwards,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingArm {
    pub direction: MappingArmDirection,
    pub lhs: Box<Spanned<Expr>>,
    pub rhs: Box<Spanned<Expr>>,
    pub lhs_pattern: Option<Spanned<Pattern>>,
    pub rhs_pattern: Option<Spanned<Pattern>>,
    pub guard: Option<Spanned<Expr>>,
    pub arrow_span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedDefKind {
    Register,
    Overload,
    Struct,
    Union,
    Bitfield,
    Enum,
    Newtype,
    Let,
    Var,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeParam {
    pub name: Spanned<String>,
    pub kind: Option<Spanned<String>>,
    pub is_constant: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeParamTail {
    Type(Spanned<TypeExpr>),
    Constraint(Spanned<TypeExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeParamSpec {
    pub params: Vec<TypeParam>,
    pub tail: Option<TypeParamTail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumMember {
    pub name: Spanned<String>,
    pub value: Option<Spanned<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumFunction {
    pub name: Spanned<String>,
    pub ty: Spanned<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeEntry {
    pub key: Spanned<String>,
    pub value: Spanned<AttributeData>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeData {
    Object(Vec<Spanned<AttributeEntry>>),
    Array(Vec<Spanned<AttributeData>>),
    Number(String),
    String(String),
    Ident(String),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamedDefDetail {
    Enum {
        members: Vec<Spanned<EnumMember>>,
        functions: Vec<Spanned<EnumFunction>>,
    },
    Struct {
        fields: Vec<Spanned<TypedField>>,
    },
    Union {
        variants: Vec<Spanned<UnionVariant>>,
    },
    Bitfield {
        fields: Vec<Spanned<BitfieldField>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedField {
    pub name: Spanned<String>,
    pub ty: Spanned<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnionVariant {
    pub name: Spanned<String>,
    pub payload: UnionPayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnionPayload {
    Type(Spanned<TypeExpr>),
    Struct { fields: Vec<Spanned<TypedField>> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitfieldField {
    pub name: Spanned<String>,
    pub high: Spanned<TypeExpr>,
    pub low: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeArrowKind {
    Function,
    Mapping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    Wild,
    Named(String),
    TypeVar(String),
    Literal(String),
    Dec,
    Inc,
    Config(Vec<Spanned<String>>),
    Forall {
        vars: Vec<TypeParam>,
        constraint: Option<Box<Spanned<TypeExpr>>>,
        body: Box<Spanned<TypeExpr>>,
    },
    Existential {
        binder: TypeParam,
        constraint: Option<Box<Spanned<TypeExpr>>>,
        body: Box<Spanned<TypeExpr>>,
    },
    Effect {
        ty: Box<Spanned<TypeExpr>>,
        effects: Vec<Spanned<String>>,
    },
    App {
        callee: Spanned<String>,
        args: Vec<Spanned<TypeExpr>>,
    },
    Tuple(Vec<Spanned<TypeExpr>>),
    Register(Box<Spanned<TypeExpr>>),
    Set(Vec<Spanned<TypeExpr>>),
    Prefix {
        op: Spanned<String>,
        ty: Box<Spanned<TypeExpr>>,
    },
    Infix {
        lhs: Box<Spanned<TypeExpr>>,
        op: Spanned<String>,
        rhs: Box<Spanned<TypeExpr>>,
    },
    Conditional {
        cond: Box<Spanned<TypeExpr>>,
        then_ty: Box<Spanned<TypeExpr>>,
        else_ty: Box<Spanned<TypeExpr>>,
    },
    Arrow {
        params: Vec<Spanned<TypeExpr>>,
        ret: Box<Spanned<TypeExpr>>,
        kind: TypeArrowKind,
    },
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Attribute {
        attr: Spanned<Attribute>,
        pattern: Box<Spanned<Pattern>>,
    },
    Wild,
    Literal(Literal),
    Ident(String),
    TypeVar(String),
    Typed(Box<Spanned<Pattern>>, Spanned<TypeExpr>),
    Tuple(Vec<Spanned<Pattern>>),
    List(Vec<Spanned<Pattern>>),
    Vector(Vec<Spanned<Pattern>>),
    App {
        callee: Spanned<String>,
        args: Vec<Spanned<Pattern>>,
    },
    Index {
        name: Spanned<String>,
        index: Spanned<TypeExpr>,
    },
    RangeIndex {
        name: Spanned<String>,
        start: Spanned<TypeExpr>,
        end: Spanned<TypeExpr>,
    },
    Struct {
        name: Option<Spanned<String>>,
        fields: Vec<Spanned<FieldPattern>>,
    },
    Infix {
        lhs: Box<Spanned<Pattern>>,
        op: Spanned<String>,
        rhs: Box<Spanned<Pattern>>,
    },
    AsBinding {
        pattern: Box<Spanned<Pattern>>,
        binding: Spanned<String>,
    },
    AsType(Box<Spanned<Pattern>>, Spanned<TypeExpr>),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldPattern {
    Binding {
        name: Spanned<String>,
        pattern: Spanned<Pattern>,
    },
    Shorthand(Spanned<String>),
    Wild(Span),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Unit,
    Number(String),
    Binary(String),
    Hex(String),
    String(String),
    Undefined,
    BitZero,
    BitOne,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBinding {
    pub pattern: Spanned<Pattern>,
    pub value: Box<Spanned<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockItem {
    Let(LetBinding),
    Var {
        target: Spanned<Expr>,
        value: Spanned<Expr>,
    },
    Expr(Spanned<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchCase {
    pub attrs: Vec<Spanned<Attribute>>,
    pub pattern: Spanned<Pattern>,
    pub guard: Option<Spanned<Expr>>,
    pub body: Spanned<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldExpr {
    Assignment {
        target: Spanned<Expr>,
        value: Spanned<Expr>,
    },
    Shorthand(Spanned<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorUpdate {
    Assignment {
        target: Spanned<Expr>,
        value: Spanned<Expr>,
    },
    RangeAssignment {
        start: Spanned<Expr>,
        end: Spanned<Expr>,
        value: Spanned<Expr>,
    },
    Shorthand(Spanned<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub callee: Box<Spanned<Expr>>,
    pub args: Vec<Spanned<Expr>>,
    pub open_span: Span,
    pub close_span: Span,
    pub arg_separator_spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeachExpr {
    pub iterator: Spanned<String>,
    pub start_keyword: Spanned<String>,
    pub start: Box<Spanned<Expr>>,
    pub end_keyword: Spanned<String>,
    pub end: Box<Spanned<Expr>>,
    pub step: Option<Box<Spanned<Expr>>>,
    pub ty: Option<Spanned<TypeExpr>>,
    pub header_span: Span,
    pub body: Box<Spanned<Expr>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixityKind {
    Infix,
    Infixl,
    Infixr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstantiationSubstitution {
    pub lhs: Spanned<String>,
    pub rhs: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminationMeasureKind {
    Function {
        pattern: Spanned<Pattern>,
        body: Spanned<Expr>,
    },
    Loop {
        measures: Vec<Spanned<LoopMeasure>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopMeasure {
    Until(Spanned<Expr>),
    Repeat(Spanned<Expr>),
    While(Spanned<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Attribute {
        attr: Spanned<Attribute>,
        expr: Box<Spanned<Expr>>,
    },
    Assign {
        lhs: Box<Spanned<Expr>>,
        rhs: Box<Spanned<Expr>>,
    },
    Let {
        binding: LetBinding,
        body: Box<Spanned<Expr>>,
    },
    Var {
        target: Box<Spanned<Expr>>,
        value: Box<Spanned<Expr>>,
        body: Box<Spanned<Expr>>,
    },
    Block(Vec<Spanned<BlockItem>>),
    Return(Box<Spanned<Expr>>),
    Throw(Box<Spanned<Expr>>),
    Assert {
        condition: Box<Spanned<Expr>>,
        message: Option<Box<Spanned<Expr>>>,
    },
    Exit(Option<Box<Spanned<Expr>>>),
    If {
        cond: Box<Spanned<Expr>>,
        then_branch: Box<Spanned<Expr>>,
        else_branch: Option<Box<Spanned<Expr>>>,
    },
    Match {
        scrutinee: Box<Spanned<Expr>>,
        cases: Vec<Spanned<MatchCase>>,
    },
    Try {
        scrutinee: Box<Spanned<Expr>>,
        cases: Vec<Spanned<MatchCase>>,
    },
    Foreach(ForeachExpr),
    Repeat {
        measure: Option<Box<Spanned<Expr>>>,
        body: Box<Spanned<Expr>>,
        until: Box<Spanned<Expr>>,
    },
    While {
        measure: Option<Box<Spanned<Expr>>>,
        cond: Box<Spanned<Expr>>,
        body: Box<Spanned<Expr>>,
    },
    Infix {
        lhs: Box<Spanned<Expr>>,
        op: Spanned<String>,
        rhs: Box<Spanned<Expr>>,
    },
    Prefix {
        op: Spanned<String>,
        expr: Box<Spanned<Expr>>,
    },
    Cast {
        expr: Box<Spanned<Expr>>,
        ty: Spanned<TypeExpr>,
    },
    Config(Vec<Spanned<String>>),
    Literal(Literal),
    Ident(String),
    TypeVar(String),
    Ref(Spanned<String>),
    Call(Call),
    Field {
        expr: Box<Spanned<Expr>>,
        field: Spanned<String>,
        via_arrow: bool,
    },
    SizeOf(Spanned<TypeExpr>),
    Constraint(Spanned<TypeExpr>),
    Index {
        expr: Box<Spanned<Expr>>,
        index: Box<Spanned<Expr>>,
    },
    Slice {
        expr: Box<Spanned<Expr>>,
        start: Box<Spanned<Expr>>,
        end: Box<Spanned<Expr>>,
    },
    VectorSlice {
        expr: Box<Spanned<Expr>>,
        start: Box<Spanned<Expr>>,
        length: Box<Spanned<Expr>>,
    },
    Struct {
        name: Option<Spanned<String>>,
        fields: Vec<Spanned<FieldExpr>>,
    },
    Update {
        base: Box<Spanned<Expr>>,
        fields: Vec<Spanned<FieldExpr>>,
    },
    List(Vec<Spanned<Expr>>),
    Vector(Vec<Spanned<Expr>>),
    VectorUpdate {
        base: Box<Spanned<Expr>>,
        updates: Vec<Spanned<VectorUpdate>>,
    },
    Tuple(Vec<Spanned<Expr>>),
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefinitionMeta {
    pub is_private: bool,
    pub attrs: Vec<Spanned<Attribute>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub meta: DefinitionMeta,
    pub kind: DefinitionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Scattered(ScatteredDefinition),
    ScatteredClause(ScatteredClauseDefinition),
    CallableSpec(CallableSpecification),
    Callable(CallableDefinition),
    TypeAlias(TypeAliasDefinition),
    Named(NamedDefinition),
    Default(DefaultDefinition),
    Fixity(FixityDefinition),
    Instantiation(InstantiationDefinition),
    Directive(DirectiveDefinition),
    End(EndDefinition),
    Constraint(ConstraintDefinition),
    TerminationMeasure(TerminationMeasureDefinition),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatteredDefinition {
    pub kind: ScatteredKind,
    pub name: Spanned<String>,
    pub params: Option<Spanned<TypeParamSpec>>,
    pub signature: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatteredClauseDefinition {
    pub kind: ScatteredClauseKind,
    pub name: Spanned<String>,
    pub member: Spanned<String>,
    pub ty: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableSpecification {
    pub kind: CallableSpecKind,
    pub name: Spanned<String>,
    pub externs: Option<Spanned<ExternSpec>>,
    pub signature: Spanned<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableDefinition {
    pub kind: CallableDefKind,
    pub name: Spanned<String>,
    pub signature: Option<Spanned<TypeExpr>>,
    pub rec_measure: Option<Spanned<RecMeasure>>,
    pub clauses: Vec<Spanned<CallableClause>>,
    pub params: Vec<Spanned<Pattern>>,
    pub return_ty: Option<Spanned<TypeExpr>>,
    pub body: Option<Spanned<Expr>>,
    pub mapping_body: Option<MappingBody>,
    pub body_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAliasDefinition {
    pub name: Spanned<String>,
    pub params: Option<Spanned<TypeParamSpec>>,
    pub kind: Option<Spanned<String>>,
    pub target: Option<Spanned<TypeExpr>>,
    pub is_operator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedDefinition {
    pub kind: NamedDefKind,
    pub name: Spanned<String>,
    pub params: Option<Spanned<TypeParamSpec>>,
    pub ty: Option<Spanned<TypeExpr>>,
    pub members: Vec<Spanned<String>>,
    pub detail: Option<NamedDefDetail>,
    pub value: Option<Spanned<Expr>>,
    pub value_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultDefinition {
    pub kind: Spanned<String>,
    pub direction: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixityDefinition {
    pub kind: FixityKind,
    pub precedence: Spanned<String>,
    pub operator: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstantiationDefinition {
    pub name: Spanned<String>,
    pub substitutions: Vec<InstantiationSubstitution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectiveDefinition {
    pub name: Spanned<String>,
    pub payload: Option<Spanned<String>>,
    pub structured_payload: Option<Spanned<AttributeData>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndDefinition {
    pub name: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintDefinition {
    pub ty: Spanned<TypeExpr>,
    pub is_type_constraint: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminationMeasureDefinition {
    pub name: Spanned<String>,
    pub kind: TerminationMeasureKind,
}

fn lower_spanned<T, U>(value: &parse_ast::Spanned<T>) -> Spanned<U>
where
    U: for<'a> From<&'a T>,
{
    (U::from(&value.0), value.1)
}

fn lower_spanned_vec<T, U>(values: &[parse_ast::Spanned<T>]) -> Vec<Spanned<U>>
where
    U: for<'a> From<&'a T>,
{
    values.iter().map(lower_spanned::<T, U>).collect()
}

fn lower_boxed_spanned<T, U>(value: &parse_ast::Spanned<T>) -> Box<Spanned<U>>
where
    U: for<'a> From<&'a T>,
{
    Box::new(lower_spanned(value))
}

impl From<&parse_ast::ScatteredKind> for ScatteredKind {
    fn from(value: &parse_ast::ScatteredKind) -> Self {
        match value {
            parse_ast::ScatteredKind::Function => Self::Function,
            parse_ast::ScatteredKind::Mapping => Self::Mapping,
            parse_ast::ScatteredKind::Union => Self::Union,
            parse_ast::ScatteredKind::Enum => Self::Enum,
        }
    }
}

impl From<&parse_ast::ScatteredClauseKind> for ScatteredClauseKind {
    fn from(value: &parse_ast::ScatteredClauseKind) -> Self {
        match value {
            parse_ast::ScatteredClauseKind::Enum => Self::Enum,
            parse_ast::ScatteredClauseKind::Union => Self::Union,
        }
    }
}

impl From<&parse_ast::CallableSpecKind> for CallableSpecKind {
    fn from(value: &parse_ast::CallableSpecKind) -> Self {
        match value {
            parse_ast::CallableSpecKind::Value => Self::Value,
            parse_ast::CallableSpecKind::Mapping => Self::Mapping,
        }
    }
}

impl From<&parse_ast::ExternBinding> for ExternBinding {
    fn from(value: &parse_ast::ExternBinding) -> Self {
        Self {
            name: value.name.clone(),
            value: value.value.clone(),
        }
    }
}

impl From<&parse_ast::ExternPurity> for ExternPurity {
    fn from(value: &parse_ast::ExternPurity) -> Self {
        match value {
            parse_ast::ExternPurity::Pure => Self::Pure,
            parse_ast::ExternPurity::Impure => Self::Impure,
        }
    }
}

impl From<&parse_ast::ExternSpec> for ExternSpec {
    fn from(value: &parse_ast::ExternSpec) -> Self {
        match value {
            parse_ast::ExternSpec::String { purity, value } => Self::String {
                purity: purity.as_ref().map(ExternPurity::from),
                value: value.clone(),
            },
            parse_ast::ExternSpec::Bindings { purity, bindings } => Self::Bindings {
                purity: purity.as_ref().map(ExternPurity::from),
                bindings: lower_spanned_vec(bindings),
            },
        }
    }
}

impl From<&parse_ast::AttributeEntry> for AttributeEntry {
    fn from(value: &parse_ast::AttributeEntry) -> Self {
        Self {
            key: value.key.clone(),
            value: lower_spanned(&value.value),
        }
    }
}

impl From<&parse_ast::AttributeData> for AttributeData {
    fn from(value: &parse_ast::AttributeData) -> Self {
        match value {
            parse_ast::AttributeData::Object(entries) => Self::Object(lower_spanned_vec(entries)),
            parse_ast::AttributeData::Array(items) => Self::Array(lower_spanned_vec(items)),
            parse_ast::AttributeData::Number(number) => Self::Number(number.clone()),
            parse_ast::AttributeData::String(text) => Self::String(text.clone()),
            parse_ast::AttributeData::Ident(ident) => Self::Ident(ident.clone()),
            parse_ast::AttributeData::Bool(value) => Self::Bool(*value),
        }
    }
}

impl From<&parse_ast::Attribute> for Attribute {
    fn from(value: &parse_ast::Attribute) -> Self {
        Self {
            name: value.name.clone(),
            data: value.data.clone(),
            parsed_data: value.parsed_data.as_ref().map(lower_spanned),
        }
    }
}

impl From<&parse_ast::DefModifiers> for DefModifiers {
    fn from(value: &parse_ast::DefModifiers) -> Self {
        Self {
            is_private: value.is_private,
            attrs: lower_spanned_vec(&value.attrs),
        }
    }
}

impl From<&parse_ast::CallableDefKind> for CallableDefKind {
    fn from(value: &parse_ast::CallableDefKind) -> Self {
        match value {
            parse_ast::CallableDefKind::Function => Self::Function,
            parse_ast::CallableDefKind::FunctionClause => Self::FunctionClause,
            parse_ast::CallableDefKind::Mapping => Self::Mapping,
            parse_ast::CallableDefKind::MappingClause => Self::MappingClause,
        }
    }
}

impl From<&parse_ast::QuantifierVar> for QuantifierVar {
    fn from(value: &parse_ast::QuantifierVar) -> Self {
        Self {
            name: value.name.clone(),
            kind: value.kind.clone(),
            is_constant: value.is_constant,
        }
    }
}

impl From<&parse_ast::CallableQuantifier> for CallableQuantifier {
    fn from(value: &parse_ast::CallableQuantifier) -> Self {
        Self {
            vars: value.vars.iter().map(QuantifierVar::from).collect(),
            constraint: value.constraint.as_ref().map(lower_spanned),
        }
    }
}

impl From<&parse_ast::RecMeasure> for RecMeasure {
    fn from(value: &parse_ast::RecMeasure) -> Self {
        Self {
            pattern: lower_spanned(&value.pattern),
            body: lower_spanned(&value.body),
        }
    }
}

impl From<&parse_ast::CallableClause> for CallableClause {
    fn from(value: &parse_ast::CallableClause) -> Self {
        Self {
            modifiers: DefModifiers::from(&value.modifiers),
            name: value.name.clone(),
            patterns: lower_spanned_vec(&value.patterns),
            guard: value.guard.as_ref().map(lower_spanned),
            quantifier: value.quantifier.as_ref().map(CallableQuantifier::from),
            return_ty: value.return_ty.as_ref().map(lower_spanned),
            body: value.body.as_ref().map(lower_spanned),
            mapping_body: value.mapping_body.as_ref().map(MappingBody::from),
            body_span: value.body_span,
        }
    }
}

impl From<&parse_ast::MappingArmDirection> for MappingArmDirection {
    fn from(value: &parse_ast::MappingArmDirection) -> Self {
        match value {
            parse_ast::MappingArmDirection::Bidirectional => Self::Bidirectional,
            parse_ast::MappingArmDirection::Forwards => Self::Forwards,
            parse_ast::MappingArmDirection::Backwards => Self::Backwards,
        }
    }
}

impl From<&parse_ast::MappingArm> for MappingArm {
    fn from(value: &parse_ast::MappingArm) -> Self {
        Self {
            direction: MappingArmDirection::from(&value.direction),
            lhs: lower_boxed_spanned(&value.lhs),
            rhs: lower_boxed_spanned(&value.rhs),
            lhs_pattern: value.lhs_pattern.as_ref().map(lower_spanned),
            rhs_pattern: value.rhs_pattern.as_ref().map(lower_spanned),
            guard: value.guard.as_ref().map(lower_spanned),
            arrow_span: value.arrow_span,
        }
    }
}

impl From<&parse_ast::MappingBody> for MappingBody {
    fn from(value: &parse_ast::MappingBody) -> Self {
        Self {
            arms: lower_spanned_vec(&value.arms),
        }
    }
}

impl From<&parse_ast::NamedDefKind> for NamedDefKind {
    fn from(value: &parse_ast::NamedDefKind) -> Self {
        match value {
            parse_ast::NamedDefKind::Register => Self::Register,
            parse_ast::NamedDefKind::Overload => Self::Overload,
            parse_ast::NamedDefKind::Struct => Self::Struct,
            parse_ast::NamedDefKind::Union => Self::Union,
            parse_ast::NamedDefKind::Bitfield => Self::Bitfield,
            parse_ast::NamedDefKind::Enum => Self::Enum,
            parse_ast::NamedDefKind::Newtype => Self::Newtype,
            parse_ast::NamedDefKind::Let => Self::Let,
            parse_ast::NamedDefKind::Var => Self::Var,
        }
    }
}

impl From<&parse_ast::TypeParam> for TypeParam {
    fn from(value: &parse_ast::TypeParam) -> Self {
        Self {
            name: value.name.clone(),
            kind: value.kind.clone(),
            is_constant: value.is_constant,
        }
    }
}

impl From<&parse_ast::TypeParamTail> for TypeParamTail {
    fn from(value: &parse_ast::TypeParamTail) -> Self {
        match value {
            parse_ast::TypeParamTail::Type(ty) => Self::Type(lower_spanned(ty)),
            parse_ast::TypeParamTail::Constraint(ty) => Self::Constraint(lower_spanned(ty)),
        }
    }
}

impl From<&parse_ast::TypeParamSpec> for TypeParamSpec {
    fn from(value: &parse_ast::TypeParamSpec) -> Self {
        Self {
            params: value.params.iter().map(TypeParam::from).collect(),
            tail: value.tail.as_ref().map(TypeParamTail::from),
        }
    }
}

impl From<&parse_ast::EnumMember> for EnumMember {
    fn from(value: &parse_ast::EnumMember) -> Self {
        Self {
            name: value.name.clone(),
            value: value.value.as_ref().map(lower_spanned),
        }
    }
}

impl From<&parse_ast::EnumFunction> for EnumFunction {
    fn from(value: &parse_ast::EnumFunction) -> Self {
        Self {
            name: value.name.clone(),
            ty: lower_spanned(&value.ty),
        }
    }
}

impl From<&parse_ast::TypedField> for TypedField {
    fn from(value: &parse_ast::TypedField) -> Self {
        Self {
            name: value.name.clone(),
            ty: lower_spanned(&value.ty),
        }
    }
}

impl From<&parse_ast::UnionPayload> for UnionPayload {
    fn from(value: &parse_ast::UnionPayload) -> Self {
        match value {
            parse_ast::UnionPayload::Type(ty) => Self::Type(lower_spanned(ty)),
            parse_ast::UnionPayload::Struct { fields } => Self::Struct {
                fields: lower_spanned_vec(fields),
            },
        }
    }
}

impl From<&parse_ast::UnionVariant> for UnionVariant {
    fn from(value: &parse_ast::UnionVariant) -> Self {
        Self {
            name: value.name.clone(),
            payload: UnionPayload::from(&value.payload),
        }
    }
}

impl From<&parse_ast::BitfieldField> for BitfieldField {
    fn from(value: &parse_ast::BitfieldField) -> Self {
        Self {
            name: value.name.clone(),
            high: lower_spanned(&value.high),
            low: value.low.as_ref().map(lower_spanned),
        }
    }
}

impl From<&parse_ast::NamedDefDetail> for NamedDefDetail {
    fn from(value: &parse_ast::NamedDefDetail) -> Self {
        match value {
            parse_ast::NamedDefDetail::Enum { members, functions } => Self::Enum {
                members: lower_spanned_vec(members),
                functions: lower_spanned_vec(functions),
            },
            parse_ast::NamedDefDetail::Struct { fields } => Self::Struct {
                fields: lower_spanned_vec(fields),
            },
            parse_ast::NamedDefDetail::Union { variants } => Self::Union {
                variants: lower_spanned_vec(variants),
            },
            parse_ast::NamedDefDetail::Bitfield { fields } => Self::Bitfield {
                fields: lower_spanned_vec(fields),
            },
        }
    }
}

impl From<&parse_ast::TypeArrowKind> for TypeArrowKind {
    fn from(value: &parse_ast::TypeArrowKind) -> Self {
        match value {
            parse_ast::TypeArrowKind::Function => Self::Function,
            parse_ast::TypeArrowKind::Mapping => Self::Mapping,
        }
    }
}

impl From<&parse_ast::TypeExpr> for TypeExpr {
    fn from(value: &parse_ast::TypeExpr) -> Self {
        match value {
            parse_ast::TypeExpr::Wild => Self::Wild,
            parse_ast::TypeExpr::Named(name) => Self::Named(name.clone()),
            parse_ast::TypeExpr::TypeVar(name) => Self::TypeVar(name.clone()),
            parse_ast::TypeExpr::Literal(text) => Self::Literal(text.clone()),
            parse_ast::TypeExpr::Dec => Self::Dec,
            parse_ast::TypeExpr::Inc => Self::Inc,
            parse_ast::TypeExpr::Config(items) => Self::Config(items.clone()),
            parse_ast::TypeExpr::Forall {
                vars,
                constraint,
                body,
            } => Self::Forall {
                vars: vars.iter().map(TypeParam::from).collect(),
                constraint: constraint.as_ref().map(|expr| lower_boxed_spanned(expr)),
                body: lower_boxed_spanned(body),
            },
            parse_ast::TypeExpr::Existential {
                binder,
                constraint,
                body,
            } => Self::Existential {
                binder: TypeParam::from(binder),
                constraint: constraint.as_ref().map(|expr| lower_boxed_spanned(expr)),
                body: lower_boxed_spanned(body),
            },
            parse_ast::TypeExpr::Effect { ty, effects } => Self::Effect {
                ty: lower_boxed_spanned(ty),
                effects: effects.clone(),
            },
            parse_ast::TypeExpr::App { callee, args } => Self::App {
                callee: callee.clone(),
                args: lower_spanned_vec(args),
            },
            parse_ast::TypeExpr::Tuple(items) => Self::Tuple(lower_spanned_vec(items)),
            parse_ast::TypeExpr::Register(ty) => Self::Register(lower_boxed_spanned(ty)),
            parse_ast::TypeExpr::Set(items) => Self::Set(lower_spanned_vec(items)),
            parse_ast::TypeExpr::Prefix { op, ty } => Self::Prefix {
                op: op.clone(),
                ty: lower_boxed_spanned(ty),
            },
            parse_ast::TypeExpr::Infix { lhs, op, rhs } => Self::Infix {
                lhs: lower_boxed_spanned(lhs),
                op: op.clone(),
                rhs: lower_boxed_spanned(rhs),
            },
            parse_ast::TypeExpr::Conditional {
                cond,
                then_ty,
                else_ty,
            } => Self::Conditional {
                cond: lower_boxed_spanned(cond),
                then_ty: lower_boxed_spanned(then_ty),
                else_ty: lower_boxed_spanned(else_ty),
            },
            parse_ast::TypeExpr::Arrow { params, ret, kind } => Self::Arrow {
                params: lower_spanned_vec(params),
                ret: lower_boxed_spanned(ret),
                kind: TypeArrowKind::from(kind),
            },
            parse_ast::TypeExpr::Error(message) => Self::Error(message.clone()),
        }
    }
}

impl From<&parse_ast::Literal> for Literal {
    fn from(value: &parse_ast::Literal) -> Self {
        match value {
            parse_ast::Literal::Bool(value) => Self::Bool(*value),
            parse_ast::Literal::Unit => Self::Unit,
            parse_ast::Literal::Number(text) => Self::Number(text.clone()),
            parse_ast::Literal::Binary(text) => Self::Binary(text.clone()),
            parse_ast::Literal::Hex(text) => Self::Hex(text.clone()),
            parse_ast::Literal::String(text) => Self::String(text.clone()),
            parse_ast::Literal::Undefined => Self::Undefined,
            parse_ast::Literal::BitZero => Self::BitZero,
            parse_ast::Literal::BitOne => Self::BitOne,
        }
    }
}

impl From<&parse_ast::FieldPattern> for FieldPattern {
    fn from(value: &parse_ast::FieldPattern) -> Self {
        match value {
            parse_ast::FieldPattern::Binding { name, pattern } => Self::Binding {
                name: name.clone(),
                pattern: lower_spanned(pattern),
            },
            parse_ast::FieldPattern::Shorthand(name) => Self::Shorthand(name.clone()),
            parse_ast::FieldPattern::Wild(span) => Self::Wild(*span),
        }
    }
}

impl From<&parse_ast::Pattern> for Pattern {
    fn from(value: &parse_ast::Pattern) -> Self {
        match value {
            parse_ast::Pattern::Attribute { attr, pattern } => Self::Attribute {
                attr: lower_spanned(attr),
                pattern: lower_boxed_spanned(pattern),
            },
            parse_ast::Pattern::Wild => Self::Wild,
            parse_ast::Pattern::Literal(literal) => Self::Literal(Literal::from(literal)),
            parse_ast::Pattern::Ident(name) => Self::Ident(name.clone()),
            parse_ast::Pattern::TypeVar(name) => Self::TypeVar(name.clone()),
            parse_ast::Pattern::Typed(pattern, ty) => {
                Self::Typed(lower_boxed_spanned(pattern), lower_spanned(ty))
            }
            parse_ast::Pattern::Tuple(items) => Self::Tuple(lower_spanned_vec(items)),
            parse_ast::Pattern::List(items) => Self::List(lower_spanned_vec(items)),
            parse_ast::Pattern::Vector(items) => Self::Vector(lower_spanned_vec(items)),
            parse_ast::Pattern::App { callee, args } => Self::App {
                callee: callee.clone(),
                args: lower_spanned_vec(args),
            },
            parse_ast::Pattern::Index { name, index } => Self::Index {
                name: name.clone(),
                index: lower_spanned(index),
            },
            parse_ast::Pattern::RangeIndex { name, start, end } => Self::RangeIndex {
                name: name.clone(),
                start: lower_spanned(start),
                end: lower_spanned(end),
            },
            parse_ast::Pattern::Struct { name, fields } => Self::Struct {
                name: name.clone(),
                fields: lower_spanned_vec(fields),
            },
            parse_ast::Pattern::Infix { lhs, op, rhs } => Self::Infix {
                lhs: lower_boxed_spanned(lhs),
                op: op.clone(),
                rhs: lower_boxed_spanned(rhs),
            },
            parse_ast::Pattern::AsBinding { pattern, binding } => Self::AsBinding {
                pattern: lower_boxed_spanned(pattern),
                binding: binding.clone(),
            },
            parse_ast::Pattern::AsType(pattern, ty) => {
                Self::AsType(lower_boxed_spanned(pattern), lower_spanned(ty))
            }
            parse_ast::Pattern::Error(message) => Self::Error(message.clone()),
        }
    }
}

impl From<&parse_ast::LetBinding> for LetBinding {
    fn from(value: &parse_ast::LetBinding) -> Self {
        Self {
            pattern: lower_spanned(&value.pattern),
            value: lower_boxed_spanned(&value.value),
        }
    }
}

impl From<&parse_ast::BlockItem> for BlockItem {
    fn from(value: &parse_ast::BlockItem) -> Self {
        match value {
            parse_ast::BlockItem::Let(binding) => Self::Let(LetBinding::from(binding)),
            parse_ast::BlockItem::Var { target, value } => Self::Var {
                target: lower_spanned(target),
                value: lower_spanned(value),
            },
            parse_ast::BlockItem::Expr(expr) => Self::Expr(lower_spanned(expr)),
        }
    }
}

impl From<&parse_ast::MatchCase> for MatchCase {
    fn from(value: &parse_ast::MatchCase) -> Self {
        Self {
            attrs: lower_spanned_vec(&value.attrs),
            pattern: lower_spanned(&value.pattern),
            guard: value.guard.as_ref().map(lower_spanned),
            body: lower_spanned(&value.body),
        }
    }
}

impl From<&parse_ast::FieldExpr> for FieldExpr {
    fn from(value: &parse_ast::FieldExpr) -> Self {
        match value {
            parse_ast::FieldExpr::Assignment { target, value } => Self::Assignment {
                target: lower_spanned(target),
                value: lower_spanned(value),
            },
            parse_ast::FieldExpr::Shorthand(name) => Self::Shorthand(name.clone()),
        }
    }
}

impl From<&parse_ast::VectorUpdate> for VectorUpdate {
    fn from(value: &parse_ast::VectorUpdate) -> Self {
        match value {
            parse_ast::VectorUpdate::Assignment { target, value } => Self::Assignment {
                target: lower_spanned(target),
                value: lower_spanned(value),
            },
            parse_ast::VectorUpdate::RangeAssignment { start, end, value } => {
                Self::RangeAssignment {
                    start: lower_spanned(start),
                    end: lower_spanned(end),
                    value: lower_spanned(value),
                }
            }
            parse_ast::VectorUpdate::Shorthand(name) => Self::Shorthand(name.clone()),
        }
    }
}

impl From<&parse_ast::Call> for Call {
    fn from(value: &parse_ast::Call) -> Self {
        Self {
            callee: lower_boxed_spanned(&value.callee),
            args: lower_spanned_vec(&value.args),
            open_span: value.open_span,
            close_span: value.close_span,
            arg_separator_spans: value.arg_separator_spans.clone(),
        }
    }
}

impl From<&parse_ast::ForeachExpr> for ForeachExpr {
    fn from(value: &parse_ast::ForeachExpr) -> Self {
        Self {
            iterator: value.iterator.clone(),
            start_keyword: value.start_keyword.clone(),
            start: lower_boxed_spanned(&value.start),
            end_keyword: value.end_keyword.clone(),
            end: lower_boxed_spanned(&value.end),
            step: value.step.as_ref().map(|expr| lower_boxed_spanned(expr)),
            ty: value.ty.as_ref().map(lower_spanned),
            header_span: value.header_span,
            body: lower_boxed_spanned(&value.body),
        }
    }
}

impl From<&parse_ast::FixityKind> for FixityKind {
    fn from(value: &parse_ast::FixityKind) -> Self {
        match value {
            parse_ast::FixityKind::Infix => Self::Infix,
            parse_ast::FixityKind::Infixl => Self::Infixl,
            parse_ast::FixityKind::Infixr => Self::Infixr,
        }
    }
}

impl From<&parse_ast::InstantiationSubstitution> for InstantiationSubstitution {
    fn from(value: &parse_ast::InstantiationSubstitution) -> Self {
        Self {
            lhs: value.lhs.clone(),
            rhs: value.rhs.clone(),
        }
    }
}

impl From<&parse_ast::LoopMeasure> for LoopMeasure {
    fn from(value: &parse_ast::LoopMeasure) -> Self {
        match value {
            parse_ast::LoopMeasure::Until(expr) => Self::Until(lower_spanned(expr)),
            parse_ast::LoopMeasure::Repeat(expr) => Self::Repeat(lower_spanned(expr)),
            parse_ast::LoopMeasure::While(expr) => Self::While(lower_spanned(expr)),
        }
    }
}

impl From<&parse_ast::TerminationMeasureKind> for TerminationMeasureKind {
    fn from(value: &parse_ast::TerminationMeasureKind) -> Self {
        match value {
            parse_ast::TerminationMeasureKind::Function { pattern, body } => Self::Function {
                pattern: lower_spanned(pattern),
                body: lower_spanned(body),
            },
            parse_ast::TerminationMeasureKind::Loop { measures } => Self::Loop {
                measures: lower_spanned_vec(measures),
            },
        }
    }
}

impl From<&parse_ast::Expr> for Expr {
    fn from(value: &parse_ast::Expr) -> Self {
        match value {
            parse_ast::Expr::Attribute { attr, expr } => Self::Attribute {
                attr: lower_spanned(attr),
                expr: lower_boxed_spanned(expr),
            },
            parse_ast::Expr::Assign { lhs, rhs } => Self::Assign {
                lhs: lower_boxed_spanned(lhs),
                rhs: lower_boxed_spanned(rhs),
            },
            parse_ast::Expr::Let { binding, body } => Self::Let {
                binding: LetBinding::from(binding),
                body: lower_boxed_spanned(body),
            },
            parse_ast::Expr::Var {
                target,
                value,
                body,
            } => Self::Var {
                target: lower_boxed_spanned(target),
                value: lower_boxed_spanned(value),
                body: lower_boxed_spanned(body),
            },
            parse_ast::Expr::Block(items) => Self::Block(lower_spanned_vec(items)),
            parse_ast::Expr::Return(expr) => Self::Return(lower_boxed_spanned(expr)),
            parse_ast::Expr::Throw(expr) => Self::Throw(lower_boxed_spanned(expr)),
            parse_ast::Expr::Assert { condition, message } => Self::Assert {
                condition: lower_boxed_spanned(condition),
                message: message.as_ref().map(|expr| lower_boxed_spanned(expr)),
            },
            parse_ast::Expr::Exit(expr) => {
                Self::Exit(expr.as_ref().map(|expr| lower_boxed_spanned(expr)))
            }
            parse_ast::Expr::If {
                cond,
                then_branch,
                else_branch,
            } => Self::If {
                cond: lower_boxed_spanned(cond),
                then_branch: lower_boxed_spanned(then_branch),
                else_branch: else_branch.as_ref().map(|expr| lower_boxed_spanned(expr)),
            },
            parse_ast::Expr::Match { scrutinee, cases } => Self::Match {
                scrutinee: lower_boxed_spanned(scrutinee),
                cases: lower_spanned_vec(cases),
            },
            parse_ast::Expr::Try { scrutinee, cases } => Self::Try {
                scrutinee: lower_boxed_spanned(scrutinee),
                cases: lower_spanned_vec(cases),
            },
            parse_ast::Expr::Foreach(foreach) => Self::Foreach(ForeachExpr::from(foreach)),
            parse_ast::Expr::Repeat {
                measure,
                body,
                until,
            } => Self::Repeat {
                measure: measure.as_ref().map(|expr| lower_boxed_spanned(expr)),
                body: lower_boxed_spanned(body),
                until: lower_boxed_spanned(until),
            },
            parse_ast::Expr::While {
                measure,
                cond,
                body,
            } => Self::While {
                measure: measure.as_ref().map(|expr| lower_boxed_spanned(expr)),
                cond: lower_boxed_spanned(cond),
                body: lower_boxed_spanned(body),
            },
            parse_ast::Expr::Infix { lhs, op, rhs } => Self::Infix {
                lhs: lower_boxed_spanned(lhs),
                op: op.clone(),
                rhs: lower_boxed_spanned(rhs),
            },
            parse_ast::Expr::Prefix { op, expr } => Self::Prefix {
                op: op.clone(),
                expr: lower_boxed_spanned(expr),
            },
            parse_ast::Expr::Cast { expr, ty } => Self::Cast {
                expr: lower_boxed_spanned(expr),
                ty: lower_spanned(ty),
            },
            parse_ast::Expr::Config(items) => Self::Config(items.clone()),
            parse_ast::Expr::Literal(literal) => Self::Literal(Literal::from(literal)),
            parse_ast::Expr::Ident(name) => Self::Ident(name.clone()),
            parse_ast::Expr::TypeVar(name) => Self::TypeVar(name.clone()),
            parse_ast::Expr::Ref(name) => Self::Ref(name.clone()),
            parse_ast::Expr::Call(call) => Self::Call(Call::from(call)),
            parse_ast::Expr::Field {
                expr,
                field,
                via_arrow,
            } => Self::Field {
                expr: lower_boxed_spanned(expr),
                field: field.clone(),
                via_arrow: *via_arrow,
            },
            parse_ast::Expr::SizeOf(ty) => Self::SizeOf(lower_spanned(ty)),
            parse_ast::Expr::Constraint(ty) => Self::Constraint(lower_spanned(ty)),
            parse_ast::Expr::Index { expr, index } => Self::Index {
                expr: lower_boxed_spanned(expr),
                index: lower_boxed_spanned(index),
            },
            parse_ast::Expr::Slice { expr, start, end } => Self::Slice {
                expr: lower_boxed_spanned(expr),
                start: lower_boxed_spanned(start),
                end: lower_boxed_spanned(end),
            },
            parse_ast::Expr::VectorSlice {
                expr,
                start,
                length,
            } => Self::VectorSlice {
                expr: lower_boxed_spanned(expr),
                start: lower_boxed_spanned(start),
                length: lower_boxed_spanned(length),
            },
            parse_ast::Expr::Struct { name, fields } => Self::Struct {
                name: name.clone(),
                fields: lower_spanned_vec(fields),
            },
            parse_ast::Expr::Update { base, fields } => Self::Update {
                base: lower_boxed_spanned(base),
                fields: lower_spanned_vec(fields),
            },
            parse_ast::Expr::List(items) => Self::List(lower_spanned_vec(items)),
            parse_ast::Expr::Vector(items) => Self::Vector(lower_spanned_vec(items)),
            parse_ast::Expr::VectorUpdate { base, updates } => Self::VectorUpdate {
                base: lower_boxed_spanned(base),
                updates: lower_spanned_vec(updates),
            },
            parse_ast::Expr::Tuple(items) => Self::Tuple(lower_spanned_vec(items)),
            parse_ast::Expr::Error(message) => Self::Error(message.clone()),
        }
    }
}

impl From<&parse_ast::DefModifiers> for DefinitionMeta {
    fn from(modifiers: &parse_ast::DefModifiers) -> Self {
        Self {
            is_private: modifiers.is_private,
            attrs: lower_spanned_vec(&modifiers.attrs),
        }
    }
}

pub(crate) fn lower_expr_spanned(value: &parse_ast::Spanned<parse_ast::Expr>) -> Spanned<Expr> {
    lower_spanned(value)
}

impl From<&parse_ast::SourceFile> for SourceFile {
    fn from(parse_file: &parse_ast::SourceFile) -> Self {
        Self {
            defs: parse_file.defs_as_core(),
        }
    }
}

trait LowerCoreDefs {
    fn defs_as_core(&self) -> Vec<Spanned<Definition>>;
}

impl LowerCoreDefs for parse_ast::SourceFile {
    fn defs_as_core(&self) -> Vec<Spanned<Definition>> {
        self.items.iter().map(lower_definition).collect()
    }
}

fn lower_definition(
    (item, span): &parse_ast::Spanned<parse_ast::TopLevelDef>,
) -> Spanned<Definition> {
    let (meta, kind) = match item {
        parse_ast::TopLevelDef::Scattered(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Scattered(ScatteredDefinition {
                kind: ScatteredKind::from(&def.kind),
                name: def.name.clone(),
                params: def.params.as_ref().map(lower_spanned),
                signature: def.signature.as_ref().map(lower_spanned),
            }),
        ),
        parse_ast::TopLevelDef::ScatteredClause(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::ScatteredClause(ScatteredClauseDefinition {
                kind: ScatteredClauseKind::from(&def.kind),
                name: def.name.clone(),
                member: def.member.clone(),
                ty: def.ty.as_ref().map(lower_spanned),
            }),
        ),
        parse_ast::TopLevelDef::CallableSpec(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::CallableSpec(CallableSpecification {
                kind: CallableSpecKind::from(&def.kind),
                name: def.name.clone(),
                externs: def.externs.as_ref().map(lower_spanned),
                signature: lower_spanned(&def.signature),
            }),
        ),
        parse_ast::TopLevelDef::CallableDef(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Callable(CallableDefinition {
                kind: CallableDefKind::from(&def.kind),
                name: def.name.clone(),
                signature: def.signature.as_ref().map(lower_spanned),
                rec_measure: def.rec_measure.as_ref().map(lower_spanned),
                clauses: lower_spanned_vec(&def.clauses),
                params: lower_spanned_vec(&def.params),
                return_ty: def.return_ty.as_ref().map(lower_spanned),
                body: def.body.as_ref().map(lower_spanned),
                mapping_body: def.mapping_body.as_ref().map(MappingBody::from),
                body_span: def.body_span,
            }),
        ),
        parse_ast::TopLevelDef::TypeAlias(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::TypeAlias(TypeAliasDefinition {
                name: def.name.clone(),
                params: def.params.as_ref().map(lower_spanned),
                kind: def.kind.clone(),
                target: def.target.as_ref().map(lower_spanned),
                is_operator: def.is_operator,
            }),
        ),
        parse_ast::TopLevelDef::Named(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Named(NamedDefinition {
                kind: NamedDefKind::from(&def.kind),
                name: def.name.clone(),
                params: def.params.as_ref().map(lower_spanned),
                ty: def.ty.as_ref().map(lower_spanned),
                members: def.members.clone(),
                detail: def.detail.as_ref().map(NamedDefDetail::from),
                value: def.value.as_ref().map(lower_spanned),
                value_span: def.value_span,
            }),
        ),
        parse_ast::TopLevelDef::Default(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Default(DefaultDefinition {
                kind: def.kind.clone(),
                direction: def.direction.clone(),
            }),
        ),
        parse_ast::TopLevelDef::Fixity(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Fixity(FixityDefinition {
                kind: FixityKind::from(&def.kind),
                precedence: def.precedence.clone(),
                operator: def.operator.clone(),
            }),
        ),
        parse_ast::TopLevelDef::Instantiation(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Instantiation(InstantiationDefinition {
                name: def.name.clone(),
                substitutions: def
                    .substitutions
                    .iter()
                    .map(InstantiationSubstitution::from)
                    .collect(),
            }),
        ),
        parse_ast::TopLevelDef::Directive(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::Directive(DirectiveDefinition {
                name: def.name.clone(),
                payload: def.payload.clone(),
                structured_payload: def.structured_payload.as_ref().map(lower_spanned),
            }),
        ),
        parse_ast::TopLevelDef::End(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::End(EndDefinition {
                name: def.name.clone(),
            }),
        ),
        parse_ast::TopLevelDef::Constraint(parse_ast::ConstraintDef {
            modifiers,
            ty,
            is_type_constraint,
        }) => (
            DefinitionMeta::from(modifiers),
            DefinitionKind::Constraint(ConstraintDefinition {
                ty: lower_spanned(ty),
                is_type_constraint: *is_type_constraint,
            }),
        ),
        parse_ast::TopLevelDef::TerminationMeasure(def) => (
            DefinitionMeta::from(&def.modifiers),
            DefinitionKind::TerminationMeasure(TerminationMeasureDefinition {
                name: def.name.clone(),
                kind: TerminationMeasureKind::from(&def.kind),
            }),
        ),
    };

    (Definition { meta, kind }, *span)
}

#[cfg(test)]
mod tests {
    use chumsky::Parser;

    #[test]
    fn lowers_parse_ast_into_core_defs() {
        let source = "private val f : unit -> unit\nfunction f() = ()\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parse_ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let core_ast = super::SourceFile::from(&parse_ast);

        assert_eq!(core_ast.defs.len(), 2);
        assert!(core_ast.defs[0].0.meta.is_private);
        match &core_ast.defs[0].0.kind {
            super::DefinitionKind::CallableSpec(spec) => assert_eq!(spec.name.0, "f"),
            other => panic!("{other:#?}"),
        }
        match &core_ast.defs[1].0.kind {
            super::DefinitionKind::Callable(def) => assert_eq!(def.name.0, "f"),
            other => panic!("{other:#?}"),
        }
    }

    #[test]
    fn lowers_nested_expr_nodes_into_core_ast() {
        let source = "function f(x : int) = if x == 0 then 1 else 2\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parse_ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let core_ast = super::SourceFile::from(&parse_ast);

        let super::DefinitionKind::Callable(def) = &core_ast.defs[0].0.kind else {
            panic!("{:#?}", core_ast.defs[0]);
        };
        let body = def.body.as_ref().expect("function body");
        assert!(matches!(body.0, super::Expr::If { .. }));
        assert!(matches!(def.params[0].0, super::Pattern::Typed(_, _)));
    }
}
