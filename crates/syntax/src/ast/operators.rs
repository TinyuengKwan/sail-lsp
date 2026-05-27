//! Defines a bunch of data-less enums for unary and binary operators.
//!
//! Types here don't know about AST, this allows re-using them for both AST and
//! HIR.

use std::fmt;

/// Unary operators.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// Logical/bitwise negation (`~` or `not`)
    Not,
    /// Arithmetic negation (`-`)
    Neg,
}

/// Binary operators — the union of all binary op kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    LogicOp(LogicOp),
    ArithOp(ArithOp),
    CmpOp(CmpOp),
    /// Concatenation (`@`) — Sail-specific
    Concat,
    /// Cons (list cons `::`) — Sail-specific
    Cons,
    /// Exponentiation (`^` in type-level numeric expressions) — Sail-specific
    Pow,
}

/// Logical operators.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LogicOp {
    And,
    Or,
}

/// Comparison operators.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq { negated: bool },
    Ord { ordering: Ordering, strict: bool },
}

/// Ordering direction for comparison operators.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ordering {
    Less,
    Greater,
}

/// Arithmetic and bitwise operators.
///
/// RA includes bitwise ops in ArithOp; Sail follows the same convention.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArithOp {
    Add,
    Mul,
    Sub,
    Div,
    Rem,
    Shl,
    Shr,
    BitXor,
    BitOr,
    BitAnd,
}

impl fmt::Display for LogicOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            LogicOp::And => "&",
            LogicOp::Or => "|",
        };
        f.write_str(res)
    }
}

impl fmt::Display for ArithOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            ArithOp::Add => "+",
            ArithOp::Mul => "*",
            ArithOp::Sub => "-",
            ArithOp::Div => "/",
            ArithOp::Rem => "%",
            ArithOp::Shl => "<<",
            ArithOp::Shr => ">>",
            ArithOp::BitXor => "^",
            ArithOp::BitOr => "|",
            ArithOp::BitAnd => "&",
        };
        f.write_str(res)
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            CmpOp::Eq { negated: false } => "==",
            CmpOp::Eq { negated: true } => "!=",
            CmpOp::Ord { ordering: Ordering::Less, strict: false } => "<=",
            CmpOp::Ord { ordering: Ordering::Less, strict: true } => "<",
            CmpOp::Ord { ordering: Ordering::Greater, strict: false } => ">=",
            CmpOp::Ord { ordering: Ordering::Greater, strict: true } => ">",
        };
        f.write_str(res)
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOp::LogicOp(op) => fmt::Display::fmt(op, f),
            BinaryOp::ArithOp(op) => fmt::Display::fmt(op, f),
            BinaryOp::CmpOp(op) => fmt::Display::fmt(op, f),
            BinaryOp::Concat => f.write_str("@"),
            BinaryOp::Cons => f.write_str("::"),
            BinaryOp::Pow => f.write_str("**"),
        }
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            UnaryOp::Not => "~",
            UnaryOp::Neg => "-",
        };
        f.write_str(res)
    }
}
