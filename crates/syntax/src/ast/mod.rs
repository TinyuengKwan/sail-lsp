//! Typed AST wrappers over rowan `SyntaxNode`.
//! Node types are generated from `sail.ungram` in `generated/nodes.rs`.
//! Hand-written accessor methods are defined alongside the generated
//! structs. The `support` module provides runtime helpers.

/// Indentation utilities and AstNodeEdit trait.
pub mod edit;
/// In-place AST editing using `ted` operations.
pub mod edit_in_place;
/// Extension methods for expression AST nodes.
pub mod expr_ext;
mod generated;
/// Free-standing AST construction functions.
pub mod make;
/// Extension methods for typed AST nodes.
pub mod node_ext;
/// Operator enums for expressions (data-less, reusable in HIR).
pub mod operators;
/// Expression precedence levels and fixity.
pub mod prec;
pub(crate) mod support;
/// Factory for creating AST nodes with optional mapping tracking.
pub mod syntax_factory;
/// Extension methods for tokens (comments, strings).
pub mod token_ext;
/// Common AST traits (HasName, HasVisibility, HasDocComments, HasAttrs).
pub mod traits;

use crate::syntax_node::{SyntaxNode, SyntaxToken};
use parser::SyntaxKind as SK;

// Re-export all generated node and token types.
pub use generated::{nodes::*, tokens::*};
// Re-export operator types ( re-export block).
pub use operators::{ArithOp, BinaryOp, CmpOp, LogicOp, Ordering, UnaryOp};

/// Trait for typed AST wrappers.
///
/// RA code calls `node.syntax().kind()` directly. Our `kind()` is used by
/// codegen for efficient enum dispatch and makes the fixed kind explicit per type.
pub trait AstNode: Sized {
    fn can_cast(kind: SK) -> bool;
    fn cast(syntax: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
    /// RA has no equivalent trait method; callers use `node.syntax().kind()`.
    fn kind() -> SK;

    /// Clone this node for in-place tree editing (via `ted`).
    fn clone_for_update(&self) -> Self
    where
        Self: Sized,
    {
        Self::cast(self.syntax().clone_for_update()).unwrap()
    }

    /// Clone this node as an independent subtree.
    fn clone_subtree(&self) -> Self
    where
        Self: Sized,
    {
        Self::cast(self.syntax().clone_subtree()).unwrap()
    }
}

/// Trait for typed token wrappers.
pub trait AstToken {
    fn can_cast(token: SK) -> bool
    where
        Self: Sized;
    fn cast(syntax: SyntaxToken) -> Option<Self>
    where
        Self: Sized;
    fn syntax(&self) -> &SyntaxToken;
    fn text(&self) -> &str {
        self.syntax().text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_text;

    #[test]
    fn source_file_cast() {
        let (root, _) = parse_text("val x : int\n");
        let sf = SourceFile::cast(root).expect("cast to SourceFile");
        // definition_nodes() gives raw SyntaxNode children
        assert!(sf.definition_nodes().count() >= 1);
    }

    #[test]
    fn callable_def_name() {
        let (root, _) = parse_text("function add(x, y) = x + y\n");
        let sf = SourceFile::cast(root).unwrap();
        let defs = sf.callable_defs();
        assert_eq!(defs.len(), 1);
        // Use name_ident() for raw IDENT token access
        let name = defs[0].name_ident().expect("name token");
        assert_eq!(name.text(), "add");
    }

    #[test]
    fn callable_spec_name() {
        let (root, _) = parse_text("val foo : int -> int\n");
        let sf = SourceFile::cast(root).unwrap();
        let specs = sf.callable_specs();
        assert_eq!(specs.len(), 1);
        let name = specs[0].name_ident().expect("name token");
        assert_eq!(name.text(), "foo");
    }

    #[test]
    fn infix_expr_structure() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let infixes: Vec<_> = root.descendants().filter_map(BinExpr::cast).collect();
        assert!(!infixes.is_empty());
        let infix = &infixes[0];
        assert!(infix.lhs().is_some());
        assert!(infix.rhs().is_some());
    }

    #[test]
    fn call_expr_structure() {
        let (root, _) = parse_text("function f() = add(1, 2)\n");
        let calls: Vec<_> = root.descendants().filter_map(CallExpr::cast).collect();
        assert!(!calls.is_empty());
        let call = &calls[0];
        assert!(call.callee().is_some());
        assert!(call.arg_list().is_some());
    }

    #[test]
    fn match_expr_arms() {
        let (root, _) = parse_text("function f(x) = match x { 0 => 10, _ => 20 }\n");
        let matches: Vec<_> = root.descendants().filter_map(MatchExpr::cast).collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].arms().len(), 2);
    }

    #[test]
    fn ident_pat_name() {
        let (root, _) = parse_text("function f(x) = match x { Some(v) => v, _ => 0 }\n");
        let pats: Vec<_> = root.descendants().filter_map(IdentPat::cast).collect();
        assert!(!pats.is_empty());
    }

    #[test]
    fn ast_node_kind() {
        assert_eq!(SourceFile::kind(), SK::SOURCE_FILE);
        assert_eq!(CallableDef::kind(), SK::CALLABLE_DEF);
        assert_eq!(BinExpr::kind(), SK::BIN_EXPR);
    }
}
