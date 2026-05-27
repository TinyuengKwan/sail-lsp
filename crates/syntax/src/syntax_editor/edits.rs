//! Predefined high-level edit operations using `SyntaxEditor`.
//! Provides trait-based convenience methods for common structural edits
//! on Sail AST nodes. Each trait adds domain-specific edit operations
//! (e.g., adding parameters, match arms, struct fields) that internally
//! use `SyntaxEditor` insert/replace/delete primitives.

use crate::ast::{self, AstNode};
use crate::syntax_node::SyntaxNode;
use crate::SyntaxKind;

use super::{Position, SyntaxEditor};

/// High-level edits for callable definitions (functions, mappings).
pub trait CallableDefEdits {
    /// Return the existing parameter list, if any.
    fn param_list(&self) -> Option<ast::ParamList>;

    /// Append a parameter node to the callable's parameter list.
    ///
    /// Does nothing if the callable has no parameter list.
    fn add_param(&self, editor: &mut SyntaxEditor, param: SyntaxNode);
}

impl CallableDefEdits for ast::CallableDef {
    fn param_list(&self) -> Option<ast::ParamList> {
        self.syntax().children().find_map(ast::ParamList::cast)
    }

    fn add_param(&self, editor: &mut SyntaxEditor, param: SyntaxNode) {
        if let Some(pl) = self.param_list() {
            let pos = Position::last_child_of(pl.syntax());
            editor.insert(pos, param);
        }
    }
}

/// High-level edits for match expressions.
pub trait MatchExprEdits {
    /// Append a match arm to this match expression.
    fn add_arm(&self, editor: &mut SyntaxEditor, arm: ast::MatchArm);

    /// Remove a match arm from this match expression.
    fn remove_arm(&self, editor: &mut SyntaxEditor, arm: &ast::MatchArm);
}

impl MatchExprEdits for ast::MatchExpr {
    fn add_arm(&self, editor: &mut SyntaxEditor, arm: ast::MatchArm) {
        // Insert after the last arm, or after the opening `{`.
        let existing_arms = self.arms();
        if let Some(last_arm) = existing_arms.last() {
            let pos = Position::after(last_arm.syntax());
            editor.insert(pos, arm.syntax().clone());
        } else {
            // No arms yet — insert as first child of the match body.
            // Find the `{` token and insert after it.
            let l_curly = self
                .syntax()
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SyntaxKind::L_CURLY);
            if let Some(brace) = l_curly {
                let pos = Position::after(brace);
                editor.insert(pos, arm.syntax().clone());
            }
        }
    }

    fn remove_arm(&self, editor: &mut SyntaxEditor, arm: &ast::MatchArm) {
        editor.delete(arm.syntax());
    }
}

/// High-level edits for named definitions (struct, union, enum, etc.).
pub trait NamedDefEdits {
    /// Append a field or variant node to this named definition.
    fn add_member(&self, editor: &mut SyntaxEditor, member: SyntaxNode);

    /// Remove a field or variant from this named definition.
    fn remove_member(&self, editor: &mut SyntaxEditor, member: &SyntaxNode);
}

impl NamedDefEdits for ast::NamedDef {
    fn add_member(&self, editor: &mut SyntaxEditor, member: SyntaxNode) {
        // Insert before the closing `}`, or at the end of the node.
        let r_curly = self
            .syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SyntaxKind::R_CURLY);
        if let Some(brace) = r_curly {
            let pos = Position::before(brace);
            editor.insert(pos, member);
        } else {
            let pos = Position::last_child_of(self.syntax());
            editor.insert(pos, member);
        }
    }

    fn remove_member(&self, editor: &mut SyntaxEditor, member: &SyntaxNode) {
        editor.delete(member);
    }
}

/// High-level edits for struct expression field initializers.
pub trait FieldInitEdits {
    /// Remove this field initializer from its parent struct expression.
    fn remove(&self, editor: &mut SyntaxEditor);
}

impl FieldInitEdits for ast::FieldInit {
    fn remove(&self, editor: &mut SyntaxEditor) {
        editor.delete(self.syntax());
    }
}
