//! Defines `ExpressionStore`: a lowered representation of expressions and
//! patterns stored in arenas with bidirectional source mapping.
//! The main `Body` struct (in `crate::body`) embeds `ExpressionStore`.
//! Lowering is handled by `ExprCollector` in `crate::body`.

pub mod body;
pub mod hir;
pub mod lower;
pub mod pretty;
pub mod scope;

use la_arena::{Arena, ArenaMap};
use rustc_hash::FxHashMap;

use la_arena::Idx;

use crate::expr_store::hir::{Expr, ExprId, Pat, PatId};
use crate::name::Name;
use crate::Span;
use syntax::SyntaxNodePtr;

/// A name binding introduced by a pattern.
///
/// that introduces a name gets a unique `BindingId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: Name,
    pub pat: PatId,
}

/// Stable ID for a binding, indexes into `ExpressionStore::bindings`.
pub type BindingId = Idx<Binding>;

/// Arena of every expression and pattern in a single callable body.
#[derive(Debug, Clone)]
pub struct ExpressionStore {
    pub exprs: Arena<Expr>,
    pub pats: Arena<Pat>,
    /// Bindings introduced by patterns.
    pub bindings: Arena<Binding>,
    /// Potential implicit declarations, keyed by the identifier l-expression.
    /// Lexical scopes activate these only for new names in block statements.
    pub assignment_bindings: ArenaMap<ExprId, PatId>,
}

impl ExpressionStore {
    pub fn exprs_len(&self) -> usize {
        self.exprs.len()
    }

    pub fn pats_len(&self) -> usize {
        self.pats.len()
    }
}

impl std::ops::Index<ExprId> for ExpressionStore {
    type Output = Expr;
    fn index(&self, id: ExprId) -> &Expr {
        &self.exprs[id]
    }
}

impl std::ops::Index<PatId> for ExpressionStore {
    type Output = Pat;
    fn index(&self, id: PatId) -> &Pat {
        &self.pats[id]
    }
}

/// Bidirectional source mapping between CST nodes and HIR IDs.
/// Forward maps use `SyntaxNodePtr` as key.
/// Reverse maps provide ExprId/PatId → SyntaxNodePtr + span.
///
/// The `_at_offset()` methods iterate the reverse map to find the
/// tightest-fitting node containing a byte offset.
#[derive(Debug, Clone, Default)]
pub struct ExpressionStoreSourceMap {
    // Forward: CST node → HIR id
    pub expr_map: FxHashMap<SyntaxNodePtr, ExprId>,
    pub pat_map: FxHashMap<SyntaxNodePtr, PatId>,
    // Reverse: HIR id → CST node
    pub expr_map_back: ArenaMap<ExprId, SyntaxNodePtr>,
    pub pat_map_back: ArenaMap<PatId, SyntaxNodePtr>,
}

impl ExpressionStoreSourceMap {
    /// Find ExprId by exact span match.
    pub fn expr_at_span(&self, span: Span) -> Option<ExprId> {
        // Walk the reverse map looking for exact match
        for (id, ptr) in self.expr_map_back.iter() {
            let range = ptr.text_range();
            if usize::from(range.start()) == span.start && usize::from(range.end()) == span.end {
                return Some(id);
            }
        }
        None
    }

    /// Find the tightest ExprId containing `offset`.
    pub fn expr_at_offset(&self, offset: usize) -> Option<ExprId> {
        let mut best: Option<(u32, ExprId)> = None; // (width, id)
        for (id, ptr) in self.expr_map_back.iter() {
            let range = ptr.text_range();
            let start = usize::from(range.start());
            let end = usize::from(range.end());
            if start <= offset && offset < end {
                let width = (end - start) as u32;
                if best.map(|(bw, _)| bw > width).unwrap_or(true) {
                    best = Some((width, id));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// Find PatId by exact span match.
    pub fn pat_at_span(&self, span: Span) -> Option<PatId> {
        for (id, ptr) in self.pat_map_back.iter() {
            let range = ptr.text_range();
            if usize::from(range.start()) == span.start && usize::from(range.end()) == span.end {
                return Some(id);
            }
        }
        None
    }

    /// Find the tightest PatId containing `offset`.
    pub fn pat_at_offset(&self, offset: usize) -> Option<PatId> {
        let mut best: Option<(u32, PatId)> = None;
        for (id, ptr) in self.pat_map_back.iter() {
            let range = ptr.text_range();
            let start = usize::from(range.start());
            let end = usize::from(range.end());
            if start <= offset && offset < end {
                let width = (end - start) as u32;
                if best.map(|(bw, _)| bw > width).unwrap_or(true) {
                    best = Some((width, id));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// ExprId → source Span.
    pub fn expr_syntax(&self, id: ExprId) -> Option<Span> {
        let ptr = self.expr_map_back.get(id)?;
        let range = ptr.text_range();
        Some(Span::new(usize::from(range.start()), usize::from(range.end())))
    }

    /// PatId → source Span.
    pub fn pat_syntax(&self, id: PatId) -> Option<Span> {
        let ptr = self.pat_map_back.get(id)?;
        let range = ptr.text_range();
        Some(Span::new(usize::from(range.start()), usize::from(range.end())))
    }

    /// ExprId → SyntaxNodePtr.
    pub fn expr_syntax_ptr(&self, id: ExprId) -> Option<SyntaxNodePtr> {
        self.expr_map_back.get(id).copied()
    }

    /// PatId → SyntaxNodePtr.
    pub fn pat_syntax_ptr(&self, id: PatId) -> Option<SyntaxNodePtr> {
        self.pat_map_back.get(id).copied()
    }

    pub fn len(&self) -> usize {
        self.expr_map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.expr_map.is_empty()
    }

    pub fn pats_len(&self) -> usize {
        self.pat_map.len()
    }
}

/// Builder that accumulates expressions and patterns during lowering,
/// then produces an `ExpressionStore` + `ExpressionStoreSourceMap`.
/// Forward/reverse maps use `SyntaxNodePtr` as the CST-side key

#[derive(Debug, Default)]
pub struct ExpressionStoreBuilder {
    pub exprs: Arena<Expr>,
    pub pats: Arena<Pat>,
    pub bindings: Arena<Binding>,
    // Forward: CST → HIR
    expr_map: FxHashMap<SyntaxNodePtr, ExprId>,
    pat_map: FxHashMap<SyntaxNodePtr, PatId>,
    // Reverse: HIR → CST
    expr_map_back: ArenaMap<ExprId, SyntaxNodePtr>,
    pat_map_back: ArenaMap<PatId, SyntaxNodePtr>,
}

impl ExpressionStoreBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate an expression with a source span (no CST node).
    ///
    /// Creates a synthetic `SyntaxNodePtr`-like entry using MISSING kind
    /// for the forward/reverse maps.
    pub fn alloc_expr(&mut self, expr: Expr, span: Span) -> ExprId {
        let id = self.exprs.alloc(expr);
        // Create a synthetic SyntaxNodePtr from the span.
        // Use MISSING kind to distinguish from real CST-lowered entries.
        let ptr = SyntaxNodePtr::from_range(
            parser::SyntaxKind::ERROR,
            rowan::TextRange::new(
                rowan::TextSize::from(span.start as u32),
                rowan::TextSize::from(span.end as u32),
            ),
        );
        self.expr_map.insert(ptr, id);
        self.expr_map_back.insert(id, ptr);
        id
    }

    /// Allocate an expression from a CST node.
    pub fn alloc_expr_cst(&mut self, expr: Expr, node: &syntax::SyntaxNode) -> ExprId {
        let ptr = SyntaxNodePtr::new(node);
        let id = self.exprs.alloc(expr);
        self.expr_map.insert(ptr, id);
        self.expr_map_back.insert(id, ptr);
        id
    }

    /// Allocate a pattern with a source span (no CST node).
    pub fn alloc_pat(&mut self, pat: Pat, span: Span) -> PatId {
        let id = self.pats.alloc(pat);
        let ptr = SyntaxNodePtr::from_range(
            parser::SyntaxKind::ERROR,
            rowan::TextRange::new(
                rowan::TextSize::from(span.start as u32),
                rowan::TextSize::from(span.end as u32),
            ),
        );
        self.pat_map.insert(ptr, id);
        self.pat_map_back.insert(id, ptr);
        id
    }

    /// Allocate a pattern from a CST node.
    pub fn alloc_pat_cst(&mut self, pat: Pat, node: &syntax::SyntaxNode) -> PatId {
        let ptr = SyntaxNodePtr::new(node);
        let id = self.pats.alloc(pat);
        self.pat_map.insert(ptr, id);
        self.pat_map_back.insert(id, ptr);
        id
    }

    /// Allocate a binding.
    pub fn alloc_binding(&mut self, name: Name, pat: PatId) -> BindingId {
        self.bindings.alloc(Binding { name, pat })
    }

    /// Consume the builder and produce the store + source map.
    pub fn finish(mut self) -> (ExpressionStore, ExpressionStoreSourceMap) {
        fn targets(exprs: &Arena<Expr>, id: ExprId, out: &mut Vec<ExprId>) {
            match &exprs[id] {
                Expr::Ident(_) => out.push(id),
                Expr::Cast { expr, .. } => targets(exprs, *expr, out),
                Expr::Tuple(items) => {
                    for &item in items {
                        targets(exprs, item, out);
                    }
                }
                _ => {}
            }
        }
        let mut candidates = Vec::new();
        for (_, expr) in self.exprs.iter() {
            if let Expr::Assign { target, .. } = expr {
                targets(&self.exprs, *target, &mut candidates);
            }
        }
        let mut assignment_bindings = ArenaMap::default();
        for id in candidates {
            let Expr::Ident(name) = &self.exprs[id] else { continue };
            let name = name.clone();
            let Some(ptr) = self.expr_map_back.get(id).copied() else { continue };
            let pat = self.pats.alloc(Pat::Bind(name.clone()));
            self.pat_map.insert(ptr, pat);
            self.pat_map_back.insert(pat, ptr);
            self.alloc_binding(Name::new(&name), pat);
            assignment_bindings.insert(id, pat);
        }
        let store = ExpressionStore {
            exprs: self.exprs,
            pats: self.pats,
            bindings: self.bindings,
            assignment_bindings,
        };
        let source_map = ExpressionStoreSourceMap {
            expr_map: self.expr_map,
            pat_map: self.pat_map,
            expr_map_back: self.expr_map_back,
            pat_map_back: self.pat_map_back,
        };
        (store, source_map)
    }
}
