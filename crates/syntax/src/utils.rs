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
