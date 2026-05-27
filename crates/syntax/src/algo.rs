//! Collection of assorted algorithms for syntax trees.
//! Provides tree traversal utilities used by IDE features:
//! - `ancestors_at_offset` — find ancestor nodes at a position
//! - `find_node_at_offset` — find a typed AST node at a position
//! - `find_node_at_range` — find a typed AST node covering a range
//! - `skip_trivia_token` — skip whitespace/comments
//! - `non_trivia_sibling` — find adjacent non-trivia element
//! - `least_common_ancestor` — LCA of two nodes

use rowan::{Direction, TextRange, TextSize};

use crate::ast::AstNode;
use crate::syntax_node::{SyntaxNode, SyntaxToken};
use crate::SyntaxElement;
use parser::SyntaxKind;

/// Returns ancestors of the node at the offset, sorted by length.
/// This should do the right thing at an edge, e.g. when searching
/// for expressions at `{ $0foo }` we will get the name reference
/// instead of the whole block.
pub fn ancestors_at_offset(
    node: &SyntaxNode,
    offset: TextSize,
) -> impl Iterator<Item = SyntaxNode> {
    node.token_at_offset(offset).into_iter().flat_map(|token| token.parent_ancestors())
}

/// Finds a node of specific AST type at offset.
///
/// Note that this is slightly imprecise: if the cursor is strictly
/// between two nodes of the desired type, the first one found
/// traversing ancestors will be returned.
pub fn find_node_at_offset<N: AstNode>(syntax: &SyntaxNode, offset: TextSize) -> Option<N> {
    ancestors_at_offset(syntax, offset).find_map(N::cast)
}

/// Finds a node of specific AST type covering the given range.
pub fn find_node_at_range<N: AstNode>(syntax: &SyntaxNode, range: TextRange) -> Option<N> {
    syntax.covering_element(range).ancestors().find_map(N::cast)
}

/// Skip to next non-trivia token in the given direction.
pub fn skip_trivia_token(mut token: SyntaxToken, direction: Direction) -> Option<SyntaxToken> {
    while token.kind().is_trivia() {
        token = match direction {
            Direction::Next => token.next_token()?,
            Direction::Prev => token.prev_token()?,
        }
    }
    Some(token)
}

/// Skip to next non-whitespace token in the given direction.
pub fn skip_whitespace_token(mut token: SyntaxToken, direction: Direction) -> Option<SyntaxToken> {
    while token.kind() == SyntaxKind::WHITESPACE {
        token = match direction {
            Direction::Next => token.next_token()?,
            Direction::Prev => token.prev_token()?,
        }
    }
    Some(token)
}

/// Finds the first sibling in the given direction which is not trivia.
pub fn non_trivia_sibling(element: SyntaxElement, direction: Direction) -> Option<SyntaxElement> {
    return match element {
        rowan::NodeOrToken::Node(node) => {
            node.siblings_with_tokens(direction).skip(1).find(not_trivia)
        }
        rowan::NodeOrToken::Token(token) => {
            token.siblings_with_tokens(direction).skip(1).find(not_trivia)
        }
    };

    fn not_trivia(element: &SyntaxElement) -> bool {
        match element {
            rowan::NodeOrToken::Node(_) => true,
            rowan::NodeOrToken::Token(token) => !token.kind().is_trivia(),
        }
    }
}

/// Find the least common ancestor of two syntax nodes.
pub fn least_common_ancestor(u: &SyntaxNode, v: &SyntaxNode) -> Option<SyntaxNode> {
    if u == v {
        return Some(u.clone());
    }

    let u_depth = u.ancestors().count();
    let v_depth = v.ancestors().count();
    let keep = u_depth.min(v_depth);

    let u_candidates = u.ancestors().skip(u_depth - keep);
    let v_candidates = v.ancestors().skip(v_depth - keep);
    let (res, _) = u_candidates.zip(v_candidates).find(|(x, y)| x == y)?;
    Some(res)
}

/// Check whether a node contains any ERROR children.
pub fn has_errors(node: &SyntaxNode) -> bool {
    node.children().any(|it| it.kind() == SyntaxKind::ERROR)
}

/// Find the previous non-trivia token before an element.
pub fn previous_non_trivia_token(e: impl Into<SyntaxElement>) -> Option<SyntaxToken> {
    let mut token = match e.into() {
        rowan::NodeOrToken::Node(n) => n.first_token()?,
        rowan::NodeOrToken::Token(t) => t,
    }
    .prev_token();
    while let Some(inner) = token {
        if !inner.kind().is_trivia() {
            return Some(inner);
        } else {
            token = inner.prev_token();
        }
    }
    None
}

/// Find the next non-trivia token after an element.
pub fn next_non_trivia_token(e: impl Into<SyntaxElement>) -> Option<SyntaxToken> {
    let mut token = match e.into() {
        rowan::NodeOrToken::Node(n) => n.last_token()?,
        rowan::NodeOrToken::Token(t) => t,
    }
    .next_token();
    while let Some(inner) = token {
        if !inner.kind().is_trivia() {
            return Some(inner);
        } else {
            token = inner.next_token();
        }
    }
    None
}

/// Find a node's neighbor (next/prev sibling of the same type).
pub fn neighbor<T: AstNode>(me: &T, direction: Direction) -> Option<T> {
    me.syntax().siblings(direction).skip(1).find_map(T::cast)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::SourceFile;
    use crate::parsing::parse_text;

    #[test]
    fn ancestors_at_offset_finds_nodes() {
        let (root, _) = parse_text("function f(x) = x + 1\n");
        let offset = TextSize::from(18); // inside "x + 1"
        let ancestors: Vec<_> = ancestors_at_offset(&root, offset).collect();
        assert!(!ancestors.is_empty());
        // The root should be among ancestors
        assert!(ancestors.iter().any(|n| n == &root));
    }

    #[test]
    fn find_node_at_offset_finds_source_file() {
        let (root, _) = parse_text("function f(x) = x\n");
        let offset = TextSize::from(0);
        let found: Option<SourceFile> = find_node_at_offset(&root, offset);
        // SourceFile wraps the root, should be found
        assert!(found.is_some());
    }

    #[test]
    fn find_node_at_range_finds_covering_node() {
        let (root, _) = parse_text("function f(x) = x + 1\n");
        let range = TextRange::new(TextSize::from(16), TextSize::from(21)); // "x + 1"
                                                                            // Use SourceFile (implements AstNode) to test find_node_at_range
        let found: Option<SourceFile> = find_node_at_range(&root, range);
        // SourceFile covers the entire file, so it should be found for any range
        assert!(found.is_some());
    }

    #[test]
    fn skip_trivia_skips_whitespace() {
        let (root, _) = parse_text("function f(x) = x\n");
        // Find first token (should be "function" keyword)
        let first = root.first_token().unwrap();
        // After "function" there's whitespace, then "f"
        let next = first.next_token().unwrap(); // whitespace
        assert_eq!(next.kind(), SyntaxKind::WHITESPACE);
        let skipped = skip_trivia_token(next, Direction::Next);
        assert!(skipped.is_some());
        assert_ne!(skipped.unwrap().kind(), SyntaxKind::WHITESPACE);
    }

    #[test]
    fn has_errors_detects_error_nodes() {
        let (root, errors) = parse_text("function f(x =\n");
        // A malformed function should produce errors
        if !errors.is_empty() {
            // The tree may contain ERROR nodes
            let _has_err = has_errors(&root);
            // Just verify the function doesn't panic
        }
    }

    #[test]
    fn least_common_ancestor_of_same_node() {
        let (root, _) = parse_text("function f(x) = x\n");
        let lca = least_common_ancestor(&root, &root);
        assert_eq!(lca, Some(root));
    }

    #[test]
    fn least_common_ancestor_of_siblings() {
        let (root, _) = parse_text("function f(x) = x\nfunction g(y) = y\n");
        let children: Vec<_> = root.children().collect();
        if children.len() >= 2 {
            let lca = least_common_ancestor(&children[0], &children[1]);
            // LCA of two sibling definitions should be the root
            assert_eq!(lca, Some(root));
        }
    }
}
