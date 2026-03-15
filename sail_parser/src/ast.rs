use crate::Span;

pub type Spanned<T> = (T, Span);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFile {
    pub items: Vec<Spanned<TopLevelDef>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopLevelDef {
    Scattered(ScatteredDef),
    ScatteredClause(ScatteredClauseDef),
    CallableSpec(CallableSpec),
    CallableDef(CallableDef),
    TypeAlias(TypeAliasDef),
    Named(NamedDef),
    Default(DefaultDef),
    Fixity(FixityDef),
    Instantiation(InstantiationDef),
    Directive(DirectiveDef),
    End(EndDef),
    Constraint(ConstraintDef),
    TerminationMeasure(TerminationMeasureDef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatteredKind {
    Function,
    Mapping,
    Union,
    Enum,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatteredDef {
    pub modifiers: DefModifiers,
    pub kind: ScatteredKind,
    pub name: Spanned<String>,
    pub params: Option<Spanned<TypeParamSpec>>,
    pub signature: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatteredClauseKind {
    Enum,
    Union,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatteredClauseDef {
    pub modifiers: DefModifiers,
    pub kind: ScatteredClauseKind,
    pub name: Spanned<String>,
    pub member: Spanned<String>,
    pub ty: Option<Spanned<TypeExpr>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallableSpecKind {
    Value,
    Mapping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableSpec {
    pub modifiers: DefModifiers,
    pub kind: CallableSpecKind,
    pub name: Spanned<String>,
    pub externs: Option<Spanned<ExternSpec>>,
    pub signature: Spanned<TypeExpr>,
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
pub struct CallableDef {
    pub modifiers: DefModifiers,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAliasDef {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
    pub params: Option<Spanned<TypeParamSpec>>,
    pub kind: Option<Spanned<String>>,
    pub target: Option<Spanned<TypeExpr>>,
    pub is_operator: bool,
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
pub struct NamedDef {
    pub modifiers: DefModifiers,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultDef {
    pub modifiers: DefModifiers,
    pub kind: Spanned<String>,
    pub direction: Spanned<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixityKind {
    Infix,
    Infixl,
    Infixr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixityDef {
    pub modifiers: DefModifiers,
    pub kind: FixityKind,
    pub precedence: Spanned<String>,
    pub operator: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstantiationSubstitution {
    pub lhs: Spanned<String>,
    pub rhs: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstantiationDef {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
    pub substitutions: Vec<InstantiationSubstitution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectiveDef {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
    pub payload: Option<Spanned<String>>,
    pub structured_payload: Option<Spanned<AttributeData>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndDef {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintDef {
    pub modifiers: DefModifiers,
    pub ty: Spanned<TypeExpr>,
    pub is_type_constraint: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminationMeasureDef {
    pub modifiers: DefModifiers,
    pub name: Spanned<String>,
    pub kind: TerminationMeasureKind,
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
