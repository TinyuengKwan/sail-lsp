//! Small utilities for working with syntax trees.

use crate::SyntaxNode;

/// Check if a syntax node is inside a comment or string literal.
pub fn is_raw_identifier(_name: &str) -> bool {
    // Sail has no raw identifiers (unlike Rust `r#ident`).
    false
}

/// Get the text of all descendant tokens joined.
pub fn node_text(node: &SyntaxNode) -> String {
    node.text().to_string()
}

/// A named application uses Sail's callable namespace, independently of a
/// same-spelled local value. Receiver fields and argument expressions do not.
pub fn is_named_callee(root: &SyntaxNode, offset: usize) -> bool {
    let offset = (offset as u32).into();
    root.token_at_offset(offset).right_biased().is_some_and(|token| {
        token.parent_ancestors().any(|node| {
            node.kind() == crate::SyntaxKind::CALL_EXPR
                && node.children().next().is_some_and(|callee| {
                    callee.kind() == crate::SyntaxKind::IDENT_EXPR
                        && callee.text_range().contains(offset)
                })
        })
    })
}
