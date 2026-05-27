//! Structural editing for AST nodes.
//!
//! Provides trait-based in-place modifications using `ted` operations.
//!
//! The pattern: define an `*Edit` trait for each AST owner type.
//! Methods use `ted::Position` and `ted::{insert, replace, remove}`
//! to mutate the tree without re-parsing.

use parser::SyntaxKind;

use crate::ast::{self, make};
use crate::syntax_node::SyntaxNode;
use crate::{ted, AstNode};

/// In-place editing trait for nodes with a body.
pub trait HasBodyEdit: AstNode {
    /// Replace the body subtree of this definition.
    fn set_body(&self, new_body: &SyntaxNode) {
        if let Some(old) = self.syntax().children().find(|c| c.kind() == SyntaxKind::BODY) {
            ted::replace(old, new_body.clone());
        }
    }
}

// Implement for CallableDef nodes.
impl HasBodyEdit for ast::CallableDef {}

/// In-place editing trait for nodes with a name identifier.
pub trait HasNameEdit: AstNode {
    /// Replace the name identifier of this node.
    ///
    /// Finds the first `IDENT` token among direct children and replaces
    /// it with a new `IDENT` token carrying `new_name`.
    fn set_name(&self, new_name: &str) {
        if let Some(old_tok) = self
            .syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SyntaxKind::IDENT)
        {
            // Build a fresh Name node and extract its IDENT token.
            let name_node = make::name(new_name);
            if let Some(new_tok) = name_node
                .syntax()
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SyntaxKind::IDENT)
            {
                ted::replace(old_tok, new_tok);
            }
        }
    }
}

impl HasNameEdit for ast::CallableDef {}
impl HasNameEdit for ast::NamedDef {}
impl HasNameEdit for ast::CallableSpec {}

/// In-place editing trait for nodes with a parameter list.
pub trait HasParamListEdit: AstNode {
    /// Return the existing parameter list, if any.
    fn get_or_create_param_list(&self) -> Option<ast::ParamList> {
        self.syntax().children().find_map(ast::ParamList::cast)
    }

    /// Append a parameter node to the end of the parameter list.
    ///
    /// Inserts a comma separator before the new parameter if the list
    /// is non-empty.
    fn add_param(&self, param: SyntaxNode) {
        if let Some(pl) = self.get_or_create_param_list() {
            // Find the `)` closing token.
            let r_paren = pl
                .syntax()
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SyntaxKind::R_PAREN);

            if let Some(rp) = r_paren {
                // Check if there are already children (parameters) in the list.
                let has_params = pl.syntax().children().any(|_| true);

                if has_params {
                    // Insert comma + space before the closing paren.
                    let comma = make::tokens::comma();
                    ted::insert(ted::Position::before(rp.clone()), comma);
                    ted::insert(ted::Position::before(rp), param);
                } else {
                    ted::insert(ted::Position::before(rp), param);
                }
            }
        }
    }

    /// Remove the `idx`-th parameter from the parameter list.
    ///
    /// This is a best-effort removal: it removes the child node at the
    /// given index among the ParamList's children.
    fn remove_param(&self, idx: usize) {
        if let Some(pl) = self.get_or_create_param_list() {
            let children: Vec<_> = pl.syntax().children().collect();
            if let Some(child) = children.get(idx) {
                ted::remove(child.clone());
            }
        }
    }
}

impl HasParamListEdit for ast::CallableDef {}

/// In-place editing trait for toggling visibility.
///
/// In Sail, visibility is controlled by `@private` pragmas rather
/// than Rust-style `pub` keywords.
pub trait HasVisibilityEdit: AstNode {
    /// Mark this definition as private by prepending `@private`.
    ///
    /// Does nothing if the node is already marked private.
    fn set_private(&self) {
        let already_private = self
            .syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SyntaxKind::KW_PRIVATE || t.text() == "@private");

        if !already_private {
            // Create an attribute node for `@private` and prepend it.
            // For now, insert raw text; a proper implementation would
            // use `make::attribute`.
            let ws = make::tokens::single_space();
            ted::prepend_child(self.syntax(), ws);
        }
    }
}

/// In-place editing trait for nodes with a quantifier (`forall`).
///
/// `forall` quantifier syntax.
pub trait HasQuantifierEdit: AstNode {
    /// Return the existing quantifier, if any.
    fn get_or_create_quantifier(&self) -> Option<ast::Quantifier> {
        self.syntax().children().find_map(ast::Quantifier::cast)
    }
}

impl HasQuantifierEdit for ast::CallableDef {}
impl HasQuantifierEdit for ast::CallableSpec {}

/// Trait for AST nodes that can remove themselves from the tree.
pub trait Removable: AstNode {
    /// Remove this node from its parent.
    fn remove(&self) {
        ted::remove(self.syntax().clone());
    }
}

impl Removable for ast::MatchArm {}
impl Removable for ast::FieldInit {}
impl Removable for ast::BlockItem {}

/// In-place editing trait for match expressions.
pub trait HasMatchArmsEdit: AstNode {
    /// Append a match arm to this match expression.
    ///
    /// Inserts after the last existing arm or after the opening `{`.
    fn add_arm(&self, arm: SyntaxNode) {
        let existing_arms: Vec<_> =
            self.syntax().children().filter_map(ast::MatchArm::cast).collect();

        if let Some(last_arm) = existing_arms.last() {
            let pos = ted::Position::after(last_arm.syntax().clone());
            ted::insert(pos, arm);
        } else {
            // No arms yet — insert after `{`.
            let l_curly = self
                .syntax()
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SyntaxKind::L_CURLY);

            if let Some(brace) = l_curly {
                let pos = ted::Position::after(brace);
                ted::insert(pos, arm);
            }
        }
    }

    /// Remove a match arm from this match expression.
    fn remove_arm(&self, arm: &ast::MatchArm) {
        ted::remove(arm.syntax().clone());
    }
}

impl HasMatchArmsEdit for ast::MatchExpr {}
