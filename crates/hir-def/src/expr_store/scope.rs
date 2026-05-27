//! Name resolution for expressions.
//! Builds a scope tree from a `Body`, mapping each `ExprId` to the
//! `ScopeId` it belongs to.  The scope tree is then used by the
//! `Resolver` for local name resolution.

use la_arena::{Arena, ArenaMap, Idx, IdxRange, RawIdx};

use crate::body::{Body, ExprId, PatId};
use crate::expr_store::hir::{Expr, Pat, Statement};
use crate::expr_store::BindingId;
use crate::name::Name;

/// Index into the scope arena.
pub type ScopeId = Idx<ScopeData>;

/// Per-body scope tree mapping `ExprId` to `ScopeId`.
#[derive(Debug, PartialEq, Eq)]
pub struct ExprScopes {
    scopes: Arena<ScopeData>,
    scope_entries: Arena<ScopeEntry>,
    scope_by_expr: ArenaMap<ExprId, ScopeId>,
}

/// One entry in a scope: a binding introduced by a pattern.
///
/// since Sail has no macros).
#[derive(Debug, PartialEq, Eq)]
pub struct ScopeEntry {
    name: Name,
    pat: PatId,
    /// Stable binding ID, if one was allocated during lowering.
    binding: Option<BindingId>,
}

impl ScopeEntry {
    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn pat(&self) -> PatId {
        self.pat
    }

    pub fn binding(&self) -> Option<BindingId> {
        self.binding
    }
}

/// What syntactic construct introduced a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeOrigin {
    /// Root scope (function parameters).
    Root,
    /// Block statement (let/var in block body).
    Block,
    /// Match arm scope.
    MatchArm,
    /// Foreach loop scope.
    ForeachLoop,
    /// Let/Var expression scope.
    LetExpr,
}

/// Data for a single scope node in the tree.
#[derive(Debug, PartialEq, Eq)]
pub struct ScopeData {
    parent: Option<ScopeId>,
    entries: IdxRange<ScopeEntry>,
    /// What introduced this scope.
    origin: ScopeOrigin,
}

impl ScopeData {
    /// The origin of this scope.
    pub fn origin(&self) -> ScopeOrigin {
        self.origin
    }
}

fn empty_entries(idx: usize) -> IdxRange<ScopeEntry> {
    let raw = Idx::from_raw(RawIdx::from_u32(idx as u32));
    IdxRange::new(raw..raw)
}

impl ExprScopes {
    /// Build scopes for a function body.
    pub fn new(body: &Body) -> Self {
        let mut scopes = ExprScopes {
            scopes: Arena::default(),
            scope_entries: Arena::default(),
            scope_by_expr: ArenaMap::with_capacity(body.len()),
        };
        let mut root = scopes.root_scope();
        // Add function parameter bindings to root scope.
        scopes.add_params_bindings(body, root, &body.params);
        // Walk the body expression tree.
        compute_expr_scopes(body.body_expr, body, &mut scopes, &mut root);
        scopes
    }

    /// Get the entries (bindings) visible in `scope`.
    pub fn entries(&self, scope: ScopeId) -> &[ScopeEntry] {
        &self.scope_entries[self.scopes[scope].entries.clone()]
    }

    /// Walk the scope chain from `scope` up to the root.
    pub fn scope_chain(&self, scope: Option<ScopeId>) -> impl Iterator<Item = ScopeId> + '_ {
        std::iter::successors(scope, move |&scope| self.scopes[scope].parent)
    }

    /// Resolve a name in a scope by walking the chain.
    pub fn resolve_name_in_scope(&self, scope: ScopeId, name: &Name) -> Option<&ScopeEntry> {
        self.scope_chain(Some(scope))
            .find_map(|scope| self.entries(scope).iter().find(|it| it.name == *name))
    }

    /// Get the innermost scope that contains `expr`.
    pub fn scope_for(&self, expr: ExprId) -> Option<ScopeId> {
        self.scope_by_expr.get(expr).copied()
    }

    /// Access the full ExprId → ScopeId mapping.
    pub fn scope_by_expr(&self) -> &ArenaMap<ExprId, ScopeId> {
        &self.scope_by_expr
    }

    /// Iterate all scopes.
    ///
    /// Used by `BodyValidationDiagnostic` to enumerate all bindings.
    pub fn all_scopes(&self) -> impl Iterator<Item = (ScopeId, &ScopeData)> + '_ {
        self.scopes.iter()
    }
}

impl ExprScopes {
    fn root_scope(&mut self) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent: None,
            entries: empty_entries(self.scope_entries.len()),
            origin: ScopeOrigin::Root,
        })
    }

    fn new_scope(&mut self, parent: ScopeId) -> ScopeId {
        self.new_scope_with_origin(parent, ScopeOrigin::Block)
    }

    fn new_scope_with_origin(&mut self, parent: ScopeId, origin: ScopeOrigin) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent: Some(parent),
            entries: empty_entries(self.scope_entries.len()),
            origin,
        })
    }

    fn add_binding(&mut self, scope: ScopeId, name: Name, pat: PatId) {
        self.add_binding_with_id(scope, name, pat, None);
    }

    fn add_binding_with_id(
        &mut self,
        scope: ScopeId,
        name: Name,
        pat: PatId,
        binding: Option<BindingId>,
    ) {
        let entry = self.scope_entries.alloc(ScopeEntry { name, pat, binding });
        self.scopes[scope].entries =
            IdxRange::new_inclusive(self.scopes[scope].entries.start()..=entry);
    }

    /// Look up the BindingId for a PatId in the body's bindings arena.
    fn find_binding_id(body: &Body, pat: PatId) -> Option<BindingId> {
        body.store.bindings.iter().find(|(_, b)| b.pat == pat).map(|(id, _)| id)
    }

    fn add_pat_bindings(&mut self, body: &Body, scope: ScopeId, pat: PatId) {
        if let Some(pattern) = body.pat(pat) {
            match pattern {
                Pat::Bind(name) => {
                    let bid = Self::find_binding_id(body, pat);
                    self.add_binding_with_id(scope, Name::new(name), pat, bid);
                }
                Pat::As { pat: inner, binding } => {
                    let bid = Self::find_binding_id(body, pat);
                    self.add_binding_with_id(scope, Name::new(binding), pat, bid);
                    self.add_pat_bindings(body, scope, *inner);
                }
                Pat::TypeVar(name) => {
                    let bid = Self::find_binding_id(body, pat);
                    self.add_binding_with_id(scope, Name::new(name), pat, bid);
                }
                Pat::Typed { inner, .. } => {
                    self.add_pat_bindings(body, scope, *inner);
                }
                Pat::Tuple(pats) | Pat::List(pats) | Pat::Array(pats) => {
                    for &p in pats {
                        self.add_pat_bindings(body, scope, p);
                    }
                }
                Pat::App { args, .. } => {
                    for &p in args {
                        self.add_pat_bindings(body, scope, p);
                    }
                }
                Pat::Struct { fields, .. } => {
                    for (_, p) in fields {
                        self.add_pat_bindings(body, scope, *p);
                    }
                }
                Pat::Infix { lhs, rhs, .. } => {
                    self.add_pat_bindings(body, scope, *lhs);
                    self.add_pat_bindings(body, scope, *rhs);
                }
                Pat::Missing
                | Pat::Wild
                | Pat::Literal(_)
                | Pat::Index { .. }
                | Pat::RangeIndex { .. }
                | Pat::AsType { .. } => {}
            }
        }
    }

    fn add_params_bindings(&mut self, body: &Body, scope: ScopeId, params: &[PatId]) {
        for &pat in params {
            self.add_pat_bindings(body, scope, pat);
        }
    }

    fn set_scope(&mut self, node: ExprId, scope: ScopeId) {
        self.scope_by_expr.insert(node, scope);
    }
}

fn compute_expr_scopes(expr: ExprId, body: &Body, scopes: &mut ExprScopes, scope: &mut ScopeId) {
    let Some(e) = body.expr(expr) else { return };
    scopes.set_scope(expr, *scope);

    match e.clone() {
        Expr::Block(stmts) => {
            compute_block_scopes(&stmts, body, scopes, scope);
        }
        Expr::Let { pat, value, body: let_body } => {
            compute_expr_scopes(value, body, scopes, scope);
            *scope = scopes.new_scope_with_origin(*scope, ScopeOrigin::LetExpr);
            scopes.add_pat_bindings(body, *scope, pat);
            compute_expr_scopes(let_body, body, scopes, scope);
        }
        Expr::Match { scrutinee, arms } | Expr::Try { scrutinee, arms } => {
            compute_expr_scopes(scrutinee, body, scopes, scope);
            for arm in &arms {
                let mut arm_scope = scopes.new_scope_with_origin(*scope, ScopeOrigin::MatchArm);
                scopes.add_pat_bindings(body, arm_scope, arm.pat);
                if let Some(guard) = arm.guard {
                    compute_expr_scopes(guard, body, scopes, &mut arm_scope);
                }
                compute_expr_scopes(arm.body, body, scopes, &mut arm_scope);
            }
        }
        // Foreach now has pat: PatId instead of iterator: String.
        Expr::Foreach { pat, start, end, step, body: loop_body } => {
            compute_expr_scopes(start, body, scopes, scope);
            compute_expr_scopes(end, body, scopes, scope);
            if let Some(step) = step {
                compute_expr_scopes(step, body, scopes, scope);
            }
            let mut loop_scope = scopes.new_scope_with_origin(*scope, ScopeOrigin::ForeachLoop);
            scopes.add_pat_bindings(body, loop_scope, pat);
            compute_expr_scopes(loop_body, body, scopes, &mut loop_scope);
        }
        Expr::If { cond, then_branch, else_branch } => {
            compute_expr_scopes(cond, body, scopes, scope);
            compute_expr_scopes(then_branch, body, scopes, scope);
            if let Some(else_branch) = else_branch {
                compute_expr_scopes(else_branch, body, scopes, scope);
            }
        }
        Expr::While { cond, body: loop_body } => {
            compute_expr_scopes(cond, body, scopes, scope);
            compute_expr_scopes(loop_body, body, scopes, scope);
        }
        Expr::Repeat { body: loop_body, until } => {
            compute_expr_scopes(loop_body, body, scopes, scope);
            compute_expr_scopes(until, body, scopes, scope);
        }
        Expr::Call { callee, args } => {
            compute_expr_scopes(callee, body, scopes, scope);
            for arg in &args {
                compute_expr_scopes(*arg, body, scopes, scope);
            }
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            compute_expr_scopes(lhs, body, scopes, scope);
            compute_expr_scopes(rhs, body, scopes, scope);
        }
        Expr::Assign { target, value } => {
            compute_expr_scopes(target, body, scopes, scope);
            compute_expr_scopes(value, body, scopes, scope);
        }
        Expr::Var { target, value, body: var_body } => {
            // Expr::Var creates a scope for the target binding,
            // mirroring Expr::Let scope handling.
            compute_expr_scopes(value, body, scopes, scope);
            let mut var_scope = scopes.new_scope(*scope);
            // Extract binding name from target (Expr::Ident)
            if let Some(Expr::Ident(name)) = body.expr(target) {
                let sentinel_pat = PatId::from_raw(RawIdx::from_u32(u32::MAX));
                scopes.add_binding(var_scope, Name::new(name), sentinel_pat);
            }
            compute_expr_scopes(var_body, body, scopes, &mut var_scope);
        }
        Expr::Index { base, index } => {
            compute_expr_scopes(base, body, scopes, scope);
            compute_expr_scopes(index, body, scopes, scope);
        }
        Expr::Subrange { base, hi, lo } => {
            compute_expr_scopes(base, body, scopes, scope);
            compute_expr_scopes(hi, body, scopes, scope);
            compute_expr_scopes(lo, body, scopes, scope);
        }
        Expr::Tuple(items) | Expr::List(items) | Expr::Array(items) => {
            for item in &items {
                compute_expr_scopes(*item, body, scopes, scope);
            }
        }
        Expr::Struct { fields, .. } => {
            for (_, expr) in &fields {
                compute_expr_scopes(*expr, body, scopes, scope);
            }
        }
        Expr::Update { base, fields } => {
            compute_expr_scopes(base, body, scopes, scope);
            for (_, expr) in &fields {
                compute_expr_scopes(*expr, body, scopes, scope);
            }
        }
        Expr::Return(e)
        | Expr::Throw(e)
        | Expr::UnaryOp { expr: e, .. }
        | Expr::Cast { expr: e, .. }
        | Expr::Field { expr: e, .. }
        | Expr::Attribute { expr: e } => {
            compute_expr_scopes(e, body, scopes, scope);
        }
        Expr::Exit(Some(e)) => {
            compute_expr_scopes(e, body, scopes, scope);
        }
        Expr::Assert { cond, message } => {
            compute_expr_scopes(cond, body, scopes, scope);
            if let Some(msg) = message {
                compute_expr_scopes(msg, body, scopes, scope);
            }
        }
        // Leaves — no child expressions.
        Expr::Missing
        | Expr::Error { .. }
        | Expr::Literal(_)
        | Expr::Ident(_)
        | Expr::TypeVar(_)
        | Expr::Ref(_)
        | Expr::Config(_)
        | Expr::SizeOf { .. }
        | Expr::Constraint(_)
        | Expr::Exit(None) => {}
    }
}

fn compute_block_scopes(
    statements: &[Statement],
    body: &Body,
    scopes: &mut ExprScopes,
    scope: &mut ScopeId,
) {
    for stmt in statements {
        match stmt {
            Statement::Let { pat, value } => {
                compute_expr_scopes(*value, body, scopes, scope);
                *scope = scopes.new_scope(*scope);
                scopes.add_pat_bindings(body, *scope, *pat);
            }
            Statement::Var { pat, value } => {
                // Var bindings create scope like Let bindings.
                compute_expr_scopes(*value, body, scopes, scope);
                *scope = scopes.new_scope(*scope);
                scopes.add_pat_bindings(body, *scope, *pat);
            }
            Statement::Expr(expr) => {
                compute_expr_scopes(*expr, body, scopes, scope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes_for(source: &str) -> (Body, ExprScopes) {
        use syntax::ast::{AstNode as _, CallableDef};
        let (root, _) = syntax::parse_text(source);
        for def in root.children().filter_map(CallableDef::cast) {
            if let Some(body_node) =
                crate::bodies::CallableBodies::cst_callable_body_node(def.syntax())
            {
                let (body, _) = Body::lower_body(&body_node);
                let scopes = ExprScopes::new(&body);
                return (body, scopes);
            }
        }
        let (body, _) = Body::lower_body(&root);
        let scopes = ExprScopes::new(&body);
        (body, scopes)
    }

    #[test]
    fn root_scope_exists() {
        let (_, scopes) = scopes_for("function f() = 42\n");
        // At least the root scope should exist
        assert!(scopes.scopes.len() >= 1);
    }

    #[test]
    fn body_root_has_scope() {
        let (body, scopes) = scopes_for("function f() = 42\n");
        let root_scope = scopes.scope_for(body.body_expr);
        assert!(root_scope.is_some());
    }

    #[test]
    fn let_creates_new_scope() {
        let (_, scopes) = scopes_for("function f() = { let x = 1; x + 2 }\n");
        // Root scope + let scope = 2 (plus possible block scope)
        assert!(scopes.scopes.len() >= 2);
    }

    #[test]
    fn match_arms_have_separate_scopes() {
        let (_, scopes) = scopes_for("function f(x) = match x { 0 => 10, _ => 20 }\n");
        // Root + 2 arm scopes = at least 3
        assert!(scopes.scopes.len() >= 3, "expected >= 3 scopes, got {}", scopes.scopes.len());
    }

    #[test]
    fn let_binding_is_resolvable() {
        let (_body, scopes) = scopes_for("function f() = { let x = 1; x + 2 }\n");
        // Find an expr inside the block (after the let)
        // The `x` in `x + 2` should be in a scope where `x` is visible
        let x_name = Name::new("x");
        let has_x =
            scopes.scopes.iter().any(|(id, _)| scopes.resolve_name_in_scope(id, &x_name).is_some());
        assert!(has_x, "expected 'x' to be resolvable in some scope");
    }

    #[test]
    fn scope_chain_reaches_root() {
        let (_, scopes) = scopes_for("function f() = { let x = 1; x }\n");
        // The innermost scope's chain should reach the root (parent = None)
        let last_scope_id = ScopeId::from_raw(RawIdx::from_u32(scopes.scopes.len() as u32 - 1));
        let chain: Vec<_> = scopes.scope_chain(Some(last_scope_id)).collect();
        assert!(chain.len() >= 1);
        // The last in chain should have no parent (root)
        let root = chain.last().unwrap();
        assert!(scopes.scopes[*root].parent.is_none());
    }
}
