//! Stable file-local syntax pointers.
//!
//! `SyntaxNodePtr` stores `(kind, range)` — same data as RA's rowan alias.
//! `AstPtr<N>` is a typed wrapper around `SyntaxNodePtr`.

use std::marker::PhantomData;

use crate::syntax_node::SyntaxNode;
use parser::SyntaxKind;
use rowan::TextRange;

use crate::ast::AstNode;

/// Stable pointer to a `SyntaxNode`. Stores `(kind, range)`.
///
/// `text_range`). RA re-exports `rowan::ast::SyntaxNodePtr` directly.
/// synthetic expressions created during CST lowering (`cst_lower`) that have
/// no live rowan node. Rowan's struct has no such raw constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyntaxNodePtr {
    kind: SyntaxKind,
    range: TextRange,
}

impl SyntaxNodePtr {
    /// Create a pointer from a live `SyntaxNode`.
    ///
    /// Mirrors rowan `SyntaxNodePtr::new`.
    pub fn new(node: &SyntaxNode) -> Self {
        Self { kind: node.kind(), range: node.text_range() }
    }

    /// Create a synthetic pointer from a kind and range.
    ///
    /// Sail-specific extension (no RA counterpart). Used when no live
    /// CST node is available (e.g., span-only lowering for synthetic exprs).
    pub fn from_range(kind: SyntaxKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// Resolve this pointer against a root node.
    ///
    /// Mirrors rowan `SyntaxNodePtr::to_node`.
    pub fn to_node(&self, root: &SyntaxNode) -> SyntaxNode {
        self.try_to_node(root).unwrap_or_else(|| {
            panic!("SyntaxNodePtr::to_node failed: {:?}@{:?}", self.kind, self.range)
        })
    }

    /// Try to resolve this pointer against a root node.
    ///
    /// Mirrors rowan `SyntaxNodePtr::try_to_node`.
    pub fn try_to_node(&self, root: &SyntaxNode) -> Option<SyntaxNode> {
        if root.parent().is_some() {
            return None;
        }
        std::iter::successors(Some(root.clone()), |node| {
            node.child_or_token_at_range(self.range)?.into_node()
        })
        .find(|it| it.text_range() == self.range && it.kind() == self.kind)
    }

    /// The kind stored in this pointer.
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// The text range stored in this pointer.
    pub fn text_range(&self) -> TextRange {
        self.range
    }
}

/// Typed version of `SyntaxNodePtr` that remembers which AST type
/// the pointer came from.
///
/// ```ignore
/// pub struct AstPtr<N: AstNode> {
///     raw: SyntaxNodePtr,
///     _ty: PhantomData<fn() -> N>,
/// }
/// ```
pub struct AstPtr<N: AstNode> {
    raw: SyntaxNodePtr,
    _ty: PhantomData<fn() -> N>,
}

// Manual impls to avoid requiring N: Clone/Debug/etc.
impl<N: AstNode> Clone for AstPtr<N> {
    fn clone(&self) -> Self {
        Self { raw: self.raw, _ty: PhantomData }
    }
}

impl<N: AstNode> std::fmt::Debug for AstPtr<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AstPtr").field("raw", &self.raw).finish()
    }
}

impl<N: AstNode> PartialEq for AstPtr<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<N: AstNode> Eq for AstPtr<N> {}

impl<N: AstNode> std::hash::Hash for AstPtr<N> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<N: AstNode> AstPtr<N> {
    /// Create a pointer from a live typed AST node.
    pub fn new(node: &N) -> Self {
        Self { raw: SyntaxNodePtr::new(node.syntax()), _ty: PhantomData }
    }

    /// Resolve to a typed AST node.
    pub fn to_node(&self, root: &SyntaxNode) -> N {
        let syntax = self.raw.to_node(root);
        N::cast(syntax).expect("AstPtr::to_node: cast failed")
    }

    /// Access the underlying raw pointer.
    pub fn syntax_node_ptr(&self) -> SyntaxNodePtr {
        self.raw
    }

    /// The text range.
    pub fn text_range(&self) -> TextRange {
        self.raw.text_range()
    }

    /// Backward-compat alias for `text_range`.
    pub fn range(&self) -> TextRange {
        self.text_range()
    }

    /// Try to cast this pointer to a different AST node type.
    pub fn cast<U: AstNode>(self) -> Option<AstPtr<U>> {
        if U::can_cast(self.raw.kind()) {
            Some(AstPtr { raw: self.raw, _ty: PhantomData })
        } else {
            None
        }
    }

    /// Try to create a typed pointer from a raw `SyntaxNodePtr`.
    pub fn try_from_raw(raw: SyntaxNodePtr) -> Option<AstPtr<N>> {
        if N::can_cast(raw.kind()) {
            Some(AstPtr { raw, _ty: PhantomData })
        } else {
            None
        }
    }
}

impl<N: AstNode> From<AstPtr<N>> for SyntaxNodePtr {
    fn from(ptr: AstPtr<N>) -> SyntaxNodePtr {
        ptr.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinExpr, SourceFile};
    use crate::parsing::parse_text;

    #[test]
    fn syntax_node_ptr_round_trip() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let callable = root.children().next().unwrap();
        let ptr = SyntaxNodePtr::new(&callable);
        let resolved = ptr.to_node(&root);
        assert_eq!(resolved.kind(), callable.kind());
        assert_eq!(resolved.text_range(), callable.text_range());
    }

    #[test]
    fn syntax_node_ptr_nested() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let infix = root.descendants().find(|n| n.kind() == SyntaxKind::BIN_EXPR);
        if let Some(infix_node) = infix {
            let ptr = SyntaxNodePtr::new(&infix_node);
            let resolved = ptr.to_node(&root);
            assert_eq!(resolved.text_range(), infix_node.text_range());
        }
    }

    #[test]
    fn ast_ptr_round_trip() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let sf = SourceFile::cast(root.clone()).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());
        let ptr = AstPtr::new(&defs[0]);
        let resolved = ptr.to_node(&root);
        assert_eq!(resolved.syntax().text_range(), defs[0].syntax().text_range());
    }

    #[test]
    fn ast_ptr_for_expr() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let infixes: Vec<BinExpr> = root.descendants().filter_map(BinExpr::cast).collect();
        if let Some(infix) = infixes.first() {
            let ptr = AstPtr::new(infix);
            let resolved = ptr.to_node(&root);
            assert_eq!(resolved.syntax().text_range(), infix.syntax().text_range());
        }
    }

    #[test]
    fn from_range_produces_valid_ptr() {
        // Verify the synthetic pointer has correct kind and range.
        let range = rowan::TextRange::new(rowan::TextSize::from(10), rowan::TextSize::from(20));
        let ptr = SyntaxNodePtr::from_range(SyntaxKind::CALLABLE_DEF, range);
        // Check that the pointer stores the correct kind and range.
        assert_eq!(ptr.kind(), SyntaxKind::CALLABLE_DEF);
        assert_eq!(ptr.text_range(), range);
    }

    #[test]
    fn from_range_matches_real_ptr() {
        // Verify that a synthetic pointer for a real node's (kind, range)
        // equals the pointer created from the node itself.
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        let callable = root.children().next().unwrap();
        let real_ptr = SyntaxNodePtr::new(&callable);
        let synth_ptr = SyntaxNodePtr::from_range(callable.kind(), callable.text_range());
        assert_eq!(real_ptr, synth_ptr);
    }
}
