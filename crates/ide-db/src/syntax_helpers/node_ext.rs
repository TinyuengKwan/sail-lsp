//! Syntax node extension methods for IDE features.
//! Higher-level helpers that combine AST + semantics for common IDE patterns.

use syntax::{SyntaxKind as SK, SyntaxNode, SyntaxToken};

/// Check if a token is inside a comment.
pub fn is_in_comment(token: &SyntaxToken) -> bool {
    matches!(token.kind(), SK::LINE_COMMENT | SK::BLOCK_COMMENT | SK::DOC_COMMENT)
}

/// Check if a token is inside a string literal.
pub fn is_in_string(token: &SyntaxToken) -> bool {
    matches!(token.kind(), SK::STRING_LIT | SK::MULTILINE_STRING_LIT)
}

/// Find the first identifier token in a node's descendants.
pub fn first_ident(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| t.kind() == SK::IDENT)
}

/// Find all identifier tokens in a node's descendants.
pub fn ident_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::IDENT)
        .collect()
}

/// Get the text of a definition node's name identifier.
pub fn def_name_text(node: &SyntaxNode) -> Option<String> {
    first_ident(node).map(|t| t.text().to_string())
}
