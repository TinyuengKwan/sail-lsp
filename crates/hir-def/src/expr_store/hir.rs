//! This module describes hir-level representation of expressions.
//! 1. Identity-based. Each expression has an `id` (`ExprId`), so we can
//!    distinguish between different `1` in `1 + 1`.
//! 2. Independent of syntax. Syntactic provenance is attached separately
//!    via id-based side map (`BodySourceMap`).
//! 3. Desugared where appropriate.
//!
//! See also the neighboring `body` module.

use std::fmt;

use la_arena::Idx;

use crate::Span;
pub use parser::Literal;
pub use syntax::ast::operators::{ArithOp, BinaryOp, CmpOp, LogicOp, Ordering, UnaryOp};

/// Structured binary operator for HIR.
///
/// with `Custom` for Sail's user-defined infix operators.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HirBinaryOp {
    /// Known operator from `syntax::ast::operators::BinaryOp`.
    Known(BinaryOp),
    /// User-defined infix operator (Sail-specific, dynamic fixity).
    Custom(String),
}

/// Structured unary operator for HIR.
///
/// with `Custom` for Sail's user-defined prefix operators.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HirUnaryOp {
    /// Known operator from `syntax::ast::operators::UnaryOp`.
    Known(UnaryOp),
    /// User-defined prefix operator (Sail-specific).
    Custom(String),
}

impl HirBinaryOp {
    /// Parse an operator string into a structured `HirBinaryOp`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(op: &str) -> Self {
        match op {
            "+" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Add)),
            "-" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Sub)),
            "*" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Mul)),
            "/" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Div)),
            "%" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Rem)),
            "<<" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Shl)),
            ">>" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::Shr)),
            "==" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Eq { negated: false })),
            "!=" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Eq { negated: true })),
            "<" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Less,
                strict: true,
            })),
            ">" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Greater,
                strict: true,
            })),
            "<=" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Less,
                strict: false,
            })),
            ">=" => HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Greater,
                strict: false,
            })),
            "&" => HirBinaryOp::Known(BinaryOp::LogicOp(LogicOp::And)),
            "|" => HirBinaryOp::Known(BinaryOp::LogicOp(LogicOp::Or)),
            "@" => HirBinaryOp::Known(BinaryOp::Concat),
            "::" => HirBinaryOp::Known(BinaryOp::Cons),
            "**" => HirBinaryOp::Known(BinaryOp::Pow),
            "^" => HirBinaryOp::Known(BinaryOp::ArithOp(ArithOp::BitXor)),
            other => HirBinaryOp::Custom(other.to_string()),
        }
    }

    /// Return the string representation of this operator.
    pub fn as_str(&self) -> &str {
        match self {
            HirBinaryOp::Known(op) => match op {
                BinaryOp::ArithOp(ArithOp::Add) => "+",
                BinaryOp::ArithOp(ArithOp::Sub) => "-",
                BinaryOp::ArithOp(ArithOp::Mul) => "*",
                BinaryOp::ArithOp(ArithOp::Div) => "/",
                BinaryOp::ArithOp(ArithOp::Rem) => "%",
                BinaryOp::ArithOp(ArithOp::Shl) => "<<",
                BinaryOp::ArithOp(ArithOp::Shr) => ">>",
                BinaryOp::ArithOp(ArithOp::BitXor) => "^",
                BinaryOp::ArithOp(ArithOp::BitOr) => "|",
                BinaryOp::ArithOp(ArithOp::BitAnd) => "&",
                BinaryOp::CmpOp(CmpOp::Eq { negated: false }) => "==",
                BinaryOp::CmpOp(CmpOp::Eq { negated: true }) => "!=",
                BinaryOp::CmpOp(CmpOp::Ord { ordering: Ordering::Less, strict: true }) => "<",
                BinaryOp::CmpOp(CmpOp::Ord { ordering: Ordering::Less, strict: false }) => "<=",
                BinaryOp::CmpOp(CmpOp::Ord { ordering: Ordering::Greater, strict: true }) => ">",
                BinaryOp::CmpOp(CmpOp::Ord { ordering: Ordering::Greater, strict: false }) => ">=",
                BinaryOp::LogicOp(LogicOp::And) => "&",
                BinaryOp::LogicOp(LogicOp::Or) => "|",
                BinaryOp::Concat => "@",
                BinaryOp::Cons => "::",
                BinaryOp::Pow => "**",
            },
            HirBinaryOp::Custom(s) => s.as_str(),
        }
    }

    /// True for comparison operators (including Sail suffixed variants).
    pub fn is_comparison(&self) -> bool {
        match self {
            HirBinaryOp::Known(BinaryOp::CmpOp(_)) => true,
            HirBinaryOp::Custom(s) => matches!(
                s.as_str(),
                "<_u"
                    | "<=_u"
                    | ">_u"
                    | ">=_u"
                    | "<_s"
                    | "<=_s"
                    | ">_s"
                    | ">=_s"
                    | "<_si"
                    | "<=_si"
                    | ">_si"
                    | ">=_si"
            ),
            _ => false,
        }
    }

    /// Check if this operator is a logical operator.
    pub fn is_logic(&self) -> bool {
        matches!(self, HirBinaryOp::Known(BinaryOp::LogicOp(_)))
    }
}

impl HirUnaryOp {
    /// Parse an operator string into a structured `HirUnaryOp`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(op: &str) -> Self {
        match op {
            "not" | "~" => HirUnaryOp::Known(UnaryOp::Not),
            "-" => HirUnaryOp::Known(UnaryOp::Neg),
            other => HirUnaryOp::Custom(other.to_string()),
        }
    }

    /// Return the string representation of this operator.
    pub fn as_str(&self) -> &str {
        match self {
            HirUnaryOp::Known(UnaryOp::Not) => "~",
            HirUnaryOp::Known(UnaryOp::Neg) => "-",
            HirUnaryOp::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for HirBinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for HirUnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable index into a [`Body`](crate::body::Body)'s expression arena.
pub type ExprId = Idx<Expr>;

/// Stable index into a [`Body`](crate::body::Body)'s pattern arena.
pub type PatId = Idx<Pat>;

/// Either an `ExprId` or `PatId`, used as key for type mismatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprOrPatId {
    ExprId(ExprId),
    PatId(PatId),
}

impl From<ExprId> for ExprOrPatId {
    fn from(id: ExprId) -> Self {
        ExprOrPatId::ExprId(id)
    }
}

impl From<PatId> for ExprOrPatId {
    fn from(id: PatId) -> Self {
        ExprOrPatId::PatId(id)
    }
}

/// Arena-native expression.  Children are IDs into the same `Body`.
#[derive(Debug, Clone)]
pub enum Expr {
    /// This is produced if the syntax tree does not have a required
    /// expression piece.
    Missing,
    /// Error recovery node — preserves error context so diagnostics
    /// can report what went wrong and IDE features (hover, completion)
    /// can still work at the error location.
    Error {
        message: String,
    },

    Literal(Literal),
    Ident(String),
    TypeVar(String),
    Ref(String),
    Config(Vec<String>),
    /// `sizeof('n)` — type-level numeric expression used as runtime value.
    /// Stores the nexp text (e.g., "'n", "'n + 1") for type inference.
    SizeOf {
        span: Span,
        nexp: String,
    },
    Constraint(Span),

    Return(ExprId),
    Throw(ExprId),
    Exit(Option<ExprId>),
    UnaryOp {
        op: HirUnaryOp,
        expr: ExprId,
    },
    Cast {
        expr: ExprId,
        ty_span: Span,
    },
    Field {
        expr: ExprId,
        field: String,
    },
    Attribute {
        expr: ExprId,
    },

    Assign {
        target: ExprId,
        value: ExprId,
    },
    BinaryOp {
        lhs: ExprId,
        op: HirBinaryOp,
        rhs: ExprId,
    },

    Let {
        pat: PatId,
        value: ExprId,
        body: ExprId,
    },
    Var {
        target: ExprId,
        value: ExprId,
        body: ExprId,
    },

    If {
        cond: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
    },
    Match {
        scrutinee: ExprId,
        arms: Vec<MatchArm>,
    },
    Try {
        scrutinee: ExprId,
        arms: Vec<MatchArm>,
    },

    While {
        cond: ExprId,
        body: ExprId,
    },
    Repeat {
        body: ExprId,
        until: ExprId,
    },
    /// `pat` is the iterator binding (was `iterator: String`).
    Foreach {
        pat: PatId,
        start: ExprId,
        end: ExprId,
        step: Option<ExprId>,
        body: ExprId,
    },

    Block(Vec<Statement>),
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    Tuple(Vec<ExprId>),
    List(Vec<ExprId>),
    Array(Vec<ExprId>),
    Struct {
        name: Option<String>,
        fields: Vec<(String, ExprId)>,
    },
    Update {
        base: ExprId,
        fields: Vec<(String, ExprId)>,
    },

    Assert {
        cond: ExprId,
        message: Option<ExprId>,
    },

    /// `v[i]` — vector/bitvector element access.
    Index {
        base: ExprId,
        index: ExprId,
    },
    /// `v[hi .. lo]` — bitvector subrange access (Sail-specific).
    Subrange {
        base: ExprId,
        hi: ExprId,
        lo: ExprId,
    },
}

/// A match/try arm: pattern + optional guard + body.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pat: PatId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}

/// Direction of a mapping arm (Sail-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingDirection {
    Forwards,
    Backwards,
    Bidirectional,
}

/// A mapping arm lowered into the Body arena (Sail-specific).
#[derive(Debug, Clone)]
pub struct MappingArm {
    pub direction: MappingDirection,
    pub lhs_pat: Option<PatId>,
    pub rhs_pat: Option<PatId>,
    pub lhs_expr: ExprId,
    pub rhs_expr: ExprId,
    pub guard: Option<ExprId>,
    pub span: crate::Span,
}

/// A block statement.
#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        pat: PatId,
        value: ExprId,
    },
    /// `var` binding now carries `PatId` (was `target: ExprId`).
    /// `var` creates a mutable binding.
    Var {
        pat: PatId,
        value: ExprId,
    },
    Expr(ExprId),
}

/// Arena-native pattern.  Children are `PatId`s into the same `Body`.
#[derive(Debug, Clone)]
pub enum Pat {
    /// Placeholder for parse errors.
    Missing,

    Wild,
    Literal(Literal),
    Bind(String),
    TypeVar(String),
    Typed {
        inner: PatId,
        ty_span: Span,
    },
    Tuple(Vec<PatId>),
    List(Vec<PatId>),
    Array(Vec<PatId>),
    App {
        ctor: String,
        args: Vec<PatId>,
    },
    Struct {
        name: Option<String>,
        fields: Vec<(String, PatId)>,
    },
    Infix {
        lhs: PatId,
        op: String,
        rhs: PatId,
    },
    As {
        pat: PatId,
        binding: String,
    },
    AsType {
        pat: PatId,
        ty_span: Span,
    },
    Index {
        name: String,
        index_span: Span,
    },
    RangeIndex {
        name: String,
        start_span: Span,
        end_span: Span,
    },
}
