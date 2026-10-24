//! `Body` arena: expressions and patterns in a callable, lowered to
//! arena-native `Expr`/`Pat` with `ExprId`/`PatId` references.

use la_arena::{Idx, RawIdx};

// ExprId and PatId are canonically defined in `crate::expr_store::hir`.
// Re-exported here for backward compatibility with `use crate::body::{ExprId, PatId}`.
pub use crate::expr_store::hir::{ExprId, PatId};

use crate::expr_store::hir::{Expr, Pat};
use crate::expr_store::{ExpressionStore, ExpressionStoreBuilder, ExpressionStoreSourceMap};
use crate::Span;

/// Create an ExprId from a raw u32 index. Used in tests.
pub fn expr_id_from_raw(raw: u32) -> ExprId {
    Idx::from_raw(RawIdx::from_u32(raw))
}

/// Create a PatId from a raw u32 index. Used in tests.
pub fn pat_id_from_raw(raw: u32) -> PatId {
    Idx::from_raw(RawIdx::from_u32(raw))
}

/// Expression/pattern arena for a single callable body.
#[derive(Debug, Clone)]
pub struct Body {
    pub store: ExpressionStore,
    pub params: Box<[PatId]>,
    pub body_expr: ExprId,
    pub mapping_arms: Vec<crate::expr_store::hir::MappingArm>,
    /// A function clause's guard: `function f (A(v) if v == 0x0) = ..`.
    ///
    /// It lives inside the parameter list as bare tokens, so nothing lowered
    /// it and no type error inside a guard was ever visible.
    pub guard: Option<ExprId>,
}

impl std::ops::Deref for Body {
    type Target = ExpressionStore;
    fn deref(&self) -> &ExpressionStore {
        &self.store
    }
}

impl Body {
    /// Create a Body from its constituent parts.
    /// Used in tests and when constructing bodies programmatically.
    pub fn new(store: ExpressionStore, params: Vec<PatId>, body_expr: ExprId) -> Self {
        Body {
            store,
            params: params.into_boxed_slice(),
            body_expr,
            mapping_arms: Vec::new(),
            guard: None,
        }
    }

    /// Create an empty Body.
    pub fn empty() -> Self {
        let mut builder = ExpressionStoreBuilder::new();
        let id = builder.alloc_expr(Expr::Missing, Span::new(0, 0));
        let (store, _source_map) = builder.finish();
        Body { store, params: Box::new([]), body_expr: id, mapping_arms: Vec::new(), guard: None }
    }

    /// Lower a CST node into a Body + BodySourceMap.
    ///
    /// Delegates to `crate::expr_store::lower::lower_body`.
    pub fn lower_body(root: &syntax::SyntaxNode) -> (Self, BodySourceMap) {
        super::lower::lower_body(root)
    }

    /// Lower body expression AND parameter patterns in one pass.
    ///
    /// Delegates to `crate::expr_store::lower::lower_body_with_params`.
    pub fn lower_body_with_params(
        root: &syntax::SyntaxNode,
        param_list_node: Option<&syntax::SyntaxNode>,
    ) -> (Self, BodySourceMap) {
        super::lower::lower_body_with_params(root, param_list_node)
    }

    pub fn len(&self) -> usize {
        self.store.exprs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.exprs.len() == 0
    }

    /// Look up an Expr by ExprId.
    pub fn expr(&self, id: ExprId) -> Option<&Expr> {
        if u32::from(id.into_raw()) < self.store.exprs.len() as u32 {
            Some(&self.store.exprs[id])
        } else {
            None
        }
    }

    /// Look up a Pat by PatId.
    pub fn pat(&self, id: PatId) -> Option<&Pat> {
        if u32::from(id.into_raw()) < self.store.pats.len() as u32 {
            Some(&self.store.pats[id])
        } else {
            None
        }
    }

    /// Deprecated stub: always returns None.
    /// Callers should use `BodySourceMap::expr_syntax()` instead.
    /// Kept temporarily for checker.rs compatibility until it's updated
    /// to receive a BodySourceMap.
    #[deprecated(note = "use BodySourceMap::expr_syntax()")]
    pub fn expr_span(&self, _id: ExprId) -> Option<Span> {
        None
    }

    /// Deprecated stub: always returns None.
    /// Callers should use `BodySourceMap::pat_syntax()` instead.
    #[deprecated(note = "use BodySourceMap::pat_syntax()")]
    pub fn pat_span(&self, _id: PatId) -> Option<Span> {
        None
    }

    pub fn pats_len(&self) -> usize {
        self.store.pats.len()
    }

    pub fn root(&self) -> ExprId {
        self.body_expr
    }

    /// Iterate all (ExprId, &Expr) pairs.
    pub fn iter_exprs(&self) -> impl Iterator<Item = (ExprId, &Expr)> + '_ {
        self.store.exprs.iter()
    }

    /// Iterate all (PatId, &Pat) pairs.
    pub fn iter_pats(&self) -> impl Iterator<Item = (PatId, &Pat)> + '_ {
        self.store.pats.iter()
    }
}

/// Index Body by ExprId — delegates to ExpressionStore.
impl std::ops::Index<ExprId> for Body {
    type Output = Expr;
    fn index(&self, id: ExprId) -> &Expr {
        &self.store[id]
    }
}

/// Index Body by PatId — delegates to ExpressionStore.
impl std::ops::Index<PatId> for Body {
    type Output = Pat;
    fn index(&self, id: PatId) -> &Pat {
        &self.store[id]
    }
}

/// Bidirectional ExprId/PatId <-> source span map for a body.
#[derive(Debug, Clone, Default)]
pub struct BodySourceMap {
    pub file_id: Option<base_db::FileId>,
    pub store: ExpressionStoreSourceMap,
    /// Where the callable starts in the file as it is *now*.
    ///
    /// `body_with_source_map` reads this fresh on every revision, which is
    /// what makes the query re-execute at all: its only other input is
    /// `callable_body`, which is deliberately backdated across an edit
    /// above the callable, and a query whose inputs all verify unchanged
    /// returns its memo — positions and all, from before the edit.
    ///
    /// It is also the datum a raw inference span needs: an
    /// `InferenceResult` that was reused records the base it was inferred
    /// with, and the difference is how far its spans have to move.
    pub def_base: usize,
}

impl BodySourceMap {
    /// Move a span recorded when the callable started at `was` to where it
    /// starts now.
    pub fn rebase(&self, span: Span, was: usize) -> Span {
        let delta = self.def_base as i64 - was as i64;
        if delta == 0 {
            return span;
        }
        let lo = (span.start as i64 + delta).max(0) as usize;
        let hi = (span.end as i64 + delta).max(0) as usize;
        Span::new(lo, hi)
    }

    pub fn expr_at_span(&self, span: Span) -> Option<ExprId> {
        self.store.expr_at_span(span)
    }

    pub fn expr_at_offset(&self, offset: usize) -> Option<ExprId> {
        self.store.expr_at_offset(offset)
    }

    pub fn pat_at_span(&self, span: Span) -> Option<PatId> {
        self.store.pat_at_span(span)
    }

    pub fn pat_at_offset(&self, offset: usize) -> Option<PatId> {
        self.store.pat_at_offset(offset)
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    pub fn pats_len(&self) -> usize {
        self.store.pats_len()
    }

    /// ExprId → SyntaxNodePtr.
    pub fn expr_syntax_ptr(&self, id: ExprId) -> Option<syntax::SyntaxNodePtr> {
        self.store.expr_syntax_ptr(id)
    }

    /// PatId → SyntaxNodePtr.
    pub fn pat_syntax_ptr(&self, id: PatId) -> Option<syntax::SyntaxNodePtr> {
        self.store.pat_syntax_ptr(id)
    }

    /// ExprId → source Span.
    pub fn expr_syntax(&self, id: ExprId) -> Option<Span> {
        self.store.expr_syntax(id)
    }

    /// PatId → source Span.
    pub fn pat_syntax(&self, id: PatId) -> Option<Span> {
        self.store.pat_syntax(id)
    }

    /// ExprId -> InFile<SyntaxNodePtr> (includes file provenance).
    pub fn expr_syntax_in_file(
        &self,
        id: ExprId,
    ) -> Option<crate::in_file::InFile<syntax::SyntaxNodePtr>> {
        let ptr = self.store.expr_syntax_ptr(id)?;
        let file_id = self.file_id?;
        Some(crate::in_file::InFile::new(file_id, ptr))
    }

    /// PatId → InFile<SyntaxNodePtr> (includes file provenance).
    pub fn pat_syntax_in_file(
        &self,
        id: PatId,
    ) -> Option<crate::in_file::InFile<syntax::SyntaxNodePtr>> {
        let ptr = self.store.pat_syntax_ptr(id)?;
        let file_id = self.file_id?;
        Some(crate::in_file::InFile::new(file_id, ptr))
    }
}
