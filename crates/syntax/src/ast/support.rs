//! Runtime support for generated AST node accessors.
//!
//! Generated methods call these to navigate the rowan tree.

use crate::syntax_node::{SyntaxNode, SyntaxToken};
use parser::SyntaxKind;

use super::AstNode;

/// Find the first child node of type `N`.
#[inline]
pub(super) fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// Find all child nodes of type `N`.
///
/// TODO: RA returns a lazy `AstChildren<N>` iterator to avoid allocation.
/// We return `Vec<N>` (eager). Consider switching to a lazy iterator type
/// to avoid heap allocation for large child lists (e.g., `SourceFile::definitions()`).
#[inline]
pub(super) fn children<N: AstNode>(parent: &SyntaxNode) -> Vec<N> {
    parent.children().filter_map(N::cast).collect()
}

/// Find the first token of a given kind among direct children.
#[inline]
#[allow(dead_code)] // Infrastructure for future generated token accessors.
pub(super) fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent.children_with_tokens().filter_map(|it| it.into_token()).find(|it| it.kind() == kind)
}

/// Find the first IDENT token among descendants.
#[inline]
pub(super) fn ident_token(parent: &SyntaxNode) -> Option<SyntaxToken> {
    parent
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SyntaxKind::IDENT)
}
