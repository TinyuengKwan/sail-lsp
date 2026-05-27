//! Expression precedence for Sail.
//! Provides precedence levels for Sail expressions, derived from the
//! parser's `binding_power` function in `grammar/mod.rs`. Used by
//! assists and formatting to determine when parentheses are required.

use parser::SyntaxKind as SK;

use crate::ast::operators::{ArithOp, BinaryOp, CmpOp, LogicOp};
use crate::syntax_node::SyntaxNode;

/// Precedence levels for Sail expressions, ordered lowest-to-highest.
///
/// The ordering matches Sail's parser binding power, so
/// `LOr < LAnd < Equality < ... < Unambiguous`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExprPrecedence {
    // Lowest
    /// `:=` assignment and `let`-in
    Assign,
    /// `|` (logical or)
    LOr,
    /// `&` (logical and)
    LAnd,
    /// `==` `!=`
    Equality,
    /// `<` `>` `<=` `>=` OP_IDENT
    Comparison,
    /// `@` (concatenation)
    Concat,
    /// `::` (cons)
    Cons,
    /// `+` `-`
    Sum,
    /// `*` `/` `%`
    Product,
    /// `^` / `**` (power, right-associative)
    Power,
    /// Unary `-` `~`
    Prefix,
    /// Field access, index, call
    Postfix,
    /// Literals, identifiers, parenthesised expressions
    Unambiguous,
    // Highest
}

/// Return the precedence of a syntax node that represents an expression.
///
/// Falls back to `Unambiguous` for unknown or atomic nodes.
pub fn precedence(expr: &SyntaxNode) -> ExprPrecedence {
    match expr.kind() {
        SK::ASSIGN_EXPR | SK::LET_EXPR | SK::VAR_EXPR => ExprPrecedence::Assign,

        SK::BIN_EXPR => bin_expr_precedence(expr),

        SK::PREFIX_EXPR => ExprPrecedence::Prefix,

        SK::CALL_EXPR | SK::FIELD_ACCESS_EXPR | SK::INDEX_EXPR => ExprPrecedence::Postfix,

        // All other expressions are self-contained / unambiguous.
        SK::LITERAL_EXPR
        | SK::IDENT_EXPR
        | SK::TUPLE_EXPR
        | SK::LIST_EXPR
        | SK::VECTOR_EXPR
        | SK::STRUCT_EXPR
        | SK::BLOCK_EXPR
        | SK::IF_EXPR
        | SK::MATCH_EXPR
        | SK::FOREACH_EXPR
        | SK::WHILE_EXPR => ExprPrecedence::Unambiguous,

        _ => ExprPrecedence::Unambiguous,
    }
}

/// Determine the precedence of a `BIN_EXPR` by inspecting its operator token.
fn bin_expr_precedence(node: &SyntaxNode) -> ExprPrecedence {
    let op_token = node
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| !t.kind().is_trivia() && !matches!(t.kind(), SK::IDENT | SK::NUM_LIT));

    let Some(tok) = op_token else {
        return ExprPrecedence::Unambiguous;
    };

    match tok.kind() {
        SK::PIPE => ExprPrecedence::LOr,
        SK::AMP => ExprPrecedence::LAnd,
        SK::EQ_EQ | SK::NEQ => ExprPrecedence::Equality,
        SK::L_ANGLE | SK::R_ANGLE | SK::LE | SK::GE | SK::OP_IDENT => ExprPrecedence::Comparison,
        SK::AT => ExprPrecedence::Concat,
        SK::SCOPE => ExprPrecedence::Cons,
        SK::PLUS | SK::MINUS => ExprPrecedence::Sum,
        SK::STAR | SK::SLASH | SK::PERCENT => ExprPrecedence::Product,
        SK::CARET => ExprPrecedence::Power,
        _ => ExprPrecedence::Unambiguous,
    }
}

/// Precedence of a classified `BinaryOp`.
pub fn binary_op_precedence(op: &BinaryOp) -> ExprPrecedence {
    match op {
        BinaryOp::LogicOp(LogicOp::Or) => ExprPrecedence::LOr,
        BinaryOp::LogicOp(LogicOp::And) => ExprPrecedence::LAnd,
        BinaryOp::CmpOp(CmpOp::Eq { .. }) => ExprPrecedence::Equality,
        BinaryOp::CmpOp(CmpOp::Ord { .. }) => ExprPrecedence::Comparison,
        BinaryOp::Concat => ExprPrecedence::Concat,
        BinaryOp::Cons => ExprPrecedence::Cons,
        BinaryOp::ArithOp(ArithOp::Add | ArithOp::Sub) => ExprPrecedence::Sum,
        BinaryOp::ArithOp(ArithOp::Mul | ArithOp::Div | ArithOp::Rem) => ExprPrecedence::Product,
        BinaryOp::ArithOp(ArithOp::Shl | ArithOp::Shr) => ExprPrecedence::Comparison,
        BinaryOp::ArithOp(ArithOp::BitXor) => ExprPrecedence::Power,
        BinaryOp::ArithOp(ArithOp::BitOr) => ExprPrecedence::LOr,
        BinaryOp::ArithOp(ArithOp::BitAnd) => ExprPrecedence::LAnd,
        BinaryOp::Pow => ExprPrecedence::Power,
    }
}

/// Associativity / fixity of an operator.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Fixity {
    Left,
    Right,
    Neither,
}

/// Return the fixity of a binary operator.
///
/// Most Sail operators are left-associative. `^` (power) and `::`
/// (cons) are right-associative. Comparison / equality operators are
/// non-associative.
pub fn fixity(op: &BinaryOp) -> Fixity {
    match op {
        BinaryOp::Pow | BinaryOp::Cons => Fixity::Right,
        BinaryOp::CmpOp(_) => Fixity::Neither,
        _ => Fixity::Left,
    }
}

impl ExprPrecedence {
    /// Returns `true` if an expression at `self` precedence needs
    /// parentheses when placed inside a context at `parent` precedence.
    pub fn needs_parentheses_in(self, parent: ExprPrecedence) -> bool {
        self < parent
    }
}
