//! Indentation utilities and `AstNodeEdit` trait.

use std::{fmt, ops};

use rowan::NodeOrToken;

use crate::{
    ast::make,
    syntax_node::{SyntaxNode, SyntaxToken},
    ted, AstNode, SyntaxKind,
};

/// Indent level, measured in units of 2 spaces (Sail convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentLevel(pub u8);

impl From<u8> for IndentLevel {
    fn from(level: u8) -> IndentLevel {
        IndentLevel(level)
    }
}

/// Sail uses 2-space indentation (RA uses 4).
const INDENT_WIDTH: usize = 2;

impl fmt::Display for IndentLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spaces = "                                        ";
        let buf;
        let len = self.0 as usize * INDENT_WIDTH;
        let indent = if len <= spaces.len() {
            &spaces[..len]
        } else {
            buf = " ".repeat(len);
            &buf
        };
        fmt::Display::fmt(indent, f)
    }
}

impl ops::Add<u8> for IndentLevel {
    type Output = IndentLevel;
    fn add(self, rhs: u8) -> IndentLevel {
        IndentLevel(self.0 + rhs)
    }
}

impl ops::AddAssign<u8> for IndentLevel {
    fn add_assign(&mut self, rhs: u8) {
        self.0 += rhs;
    }
}

impl IndentLevel {
    pub fn zero() -> IndentLevel {
        IndentLevel(0)
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    /// Infer indent level from a syntax element.
    pub fn from_element(element: &rowan::NodeOrToken<SyntaxNode, SyntaxToken>) -> IndentLevel {
        match element {
            rowan::NodeOrToken::Node(it) => IndentLevel::from_node(it),
            rowan::NodeOrToken::Token(it) => IndentLevel::from_token(it),
        }
    }

    /// Infer indent level from a node's first token.
    pub fn from_node(node: &SyntaxNode) -> IndentLevel {
        match node.first_token() {
            Some(it) => Self::from_token(&it),
            None => IndentLevel(0),
        }
    }

    /// Infer indent level by scanning backwards for preceding whitespace.
    pub fn from_token(token: &SyntaxToken) -> IndentLevel {
        // Walk backwards through preceding tokens looking for whitespace
        // containing a newline.
        let mut tok = token.prev_token();
        while let Some(ws) = tok {
            if ws.kind() == SyntaxKind::WHITESPACE {
                let text = ws.text();
                if let Some(pos) = text.rfind('\n') {
                    let level = text[pos + 1..].chars().count() / INDENT_WIDTH;
                    return IndentLevel(level as u8);
                }
            }
            tok = ws.prev_token();
        }
        IndentLevel(0)
    }

    /// Increase indentation of all newlines in `node`.
    pub(super) fn increase_indent(self, node: &SyntaxNode) {
        let tokens = node.preorder_with_tokens().filter_map(|event| match event {
            rowan::WalkEvent::Leave(NodeOrToken::Token(it)) => Some(it),
            _ => None,
        });
        for token in tokens {
            if token.kind() == SyntaxKind::WHITESPACE && token.text().contains('\n') {
                let new_ws = make::tokens::whitespace(&format!("{}{self}", token.text()));
                ted::replace(token, new_ws);
            }
        }
    }

    /// Decrease indentation of all newlines in `node`.
    pub(super) fn decrease_indent(self, node: &SyntaxNode) {
        let tokens = node.preorder_with_tokens().filter_map(|event| match event {
            rowan::WalkEvent::Leave(NodeOrToken::Token(it)) => Some(it),
            _ => None,
        });
        for token in tokens {
            if token.kind() == SyntaxKind::WHITESPACE && token.text().contains('\n') {
                let new_ws =
                    make::tokens::whitespace(&token.text().replace(&format!("\n{self}"), "\n"));
                ted::replace(token, new_ws);
            }
        }
    }

    /// Clone `node` and increase indentation in the clone.
    pub(super) fn clone_increase_indent(self, node: &SyntaxNode) -> SyntaxNode {
        let cloned = node.clone_subtree().clone_for_update();
        self.increase_indent(&cloned);
        cloned
    }

    /// Clone `node` and decrease indentation in the clone.
    pub(super) fn clone_decrease_indent(self, node: &SyntaxNode) -> SyntaxNode {
        let cloned = node.clone_subtree().clone_for_update();
        self.decrease_indent(&cloned);
        cloned
    }
}

/// Convenience trait for indenting/dedenting AST nodes.
pub trait AstNodeEdit: AstNode + Clone + Sized {
    fn indent_level(&self) -> IndentLevel {
        IndentLevel::from_node(self.syntax())
    }

    #[must_use]
    fn indent(&self, level: IndentLevel) -> Self {
        Self::cast(level.clone_increase_indent(self.syntax())).unwrap()
    }

    #[must_use]
    fn dedent(&self, level: IndentLevel) -> Self {
        Self::cast(level.clone_decrease_indent(self.syntax())).unwrap()
    }

    #[must_use]
    fn reset_indent(&self) -> Self {
        let level = IndentLevel::from_node(self.syntax());
        self.dedent(level)
    }
}

/// Blanket implementation for all AstNode + Clone types.
impl<N: AstNode + Clone> AstNodeEdit for N {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast;

    #[test]
    fn indent_level_display() {
        assert_eq!(format!("{}", IndentLevel(0)), "");
        assert_eq!(format!("{}", IndentLevel(1)), "  ");
        assert_eq!(format!("{}", IndentLevel(2)), "    ");
        assert_eq!(format!("{}", IndentLevel(3)), "      ");
    }

    #[test]
    fn indent_level_add() {
        assert_eq!(IndentLevel(1) + 2, IndentLevel(3));
    }

    #[test]
    fn indent_level_from_token_basic() {
        let parse = ast::SourceFile::parse("  val x : int\n");
        let first_token = parse.syntax_node().first_token().unwrap();
        // First token is whitespace "  ", no newline before it.
        let level = IndentLevel::from_token(&first_token);
        assert_eq!(level, IndentLevel(0));
    }
}
