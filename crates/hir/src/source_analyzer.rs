//! Maps syntax-level positions to semantic information.
//! A `SourceAnalyzer` is created for a specific position inside a
//! callable body (or at module level) and provides queries like
//! "what is the type of this expression?" or "what does this name
//! resolve to?". It bridges the syntax world (byte offsets, CST
//! nodes) with the semantic world (ExprId, Ty, Resolver).
//!
//! No lifetime parameter because we store owned data. Single `Body`
//! variant of `BodyOrSig` (Sail has no variant fields / generic sigs).

use either::Either;
use hir_def::body::{Body, BodySourceMap, ExprId, PatId};
use hir_def::nameres::DefMap;
use hir_def::resolver::Resolver;
use hir_ty::infer::{InferenceResult, Ty};

use std::sync::Arc;

/// Semantic data for a callable body or signature.
///
/// Only `Body` variant (Sail has no variant fields / generic sigs).
#[derive(Clone)]
pub(crate) enum BodyOrSig {
    Body { body: Body, source_map: BodySourceMap, infer: Option<InferenceResult> },
}

/// Core analysis engine for a specific position in source code.
#[derive(Clone)]
pub struct SourceAnalyzer {
    /// Use `make_resolver()` to derive a borrowing Resolver on demand.
    pub(crate) resolver: Arc<DefMap>,
    /// Body/sig data (None if at module level).
    pub(crate) body_or_sig: Option<BodyOrSig>,
}

impl SourceAnalyzer {
    pub(crate) fn local_at(
        &self,
        db: &dyn salsa::Database,
        file: base_db::FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::Local> {
        let BodyOrSig::Body { body, source_map, .. } = self.body_or_sig.as_ref()?;
        let scopes = hir_def::source_order::bindings::body_scopes(db, file, body, offset);
        let key = hir_def::Name::new(name);
        let pat = body.iter_exprs().find_map(|(id, expr)| {
            if !matches!(expr, hir_def::Expr::Ident(n) if n == name) {
                return None;
            }
            let span = source_map.expr_syntax(id)?;
            if !(span.start <= offset && offset < span.end) {
                return None;
            }
            Some(scopes.resolve_name_in_scope(scopes.scope_for(id)?, &key)?.pat())
        })?;
        let span = source_map.pat_syntax(pat)?;
        let is_param = body.params.iter().any(|p| {
            source_map.pat_syntax(*p).is_some_and(|s| s.start <= span.start && span.end <= s.end)
        });
        Some(crate::Local { name: key, span, is_param })
    }

    /// Create a SourceAnalyzer at module level (no body context).
    /// Used when the cursor is outside any callable body.
    pub(crate) fn new_for_resolver(resolver: Arc<DefMap>) -> Self {
        Self { resolver, body_or_sig: None }
    }

    /// Construct a borrowing `Resolver` from the stored `Arc<DefMap>`.
    pub(crate) fn make_resolver(&self) -> Resolver<'_> {
        Resolver::for_file(&self.resolver)
    }
}

impl SourceAnalyzer {
    /// Every local binding visible at `offset`, innermost first.
    ///
    /// `ExprScopes` — 466 lines with `scope_for`, `scope_chain` and
    /// `entries` — was built for exactly this and had one caller,
    /// hir-ty's unused-variable check. The resolver a `SemanticsScope`
    /// hands out is `Resolver::for_file`, which has no expression scope
    /// in it at all, so `names_in_scope` answered with the file's
    /// top-level names and nothing else: completion inside a function
    /// offered no local, no parameter and no `let`.
    pub(crate) fn local_names_at(
        &self,
        db: &dyn salsa::Database,
        file: base_db::FileText,
        offset: usize,
    ) -> Vec<hir_def::Name> {
        let Some(BodyOrSig::Body { body, source_map, .. }) = &self.body_or_sig else {
            return Vec::new();
        };
        let scopes = hir_def::source_order::bindings::body_scopes(db, file, body, offset);
        // The scope of the expression *nearest before* the cursor, not the
        // tightest one containing it. Half-typed text is often not an
        // expression the lowering kept a span for, and the tightest
        // container is then the whole block — whose scope is the root, so
        // every `let` in it is invisible. rust-analyzer's
        // `scope_for_offset` picks the closest preceding node for the same
        // reason.
        let mut best: Option<(usize, usize, hir_def::expr_store::scope::ScopeId)> = None;
        for (expr, scope) in scopes.scope_by_expr().iter() {
            let Some(span) = source_map.expr_syntax(expr) else { continue };
            if span.start > offset {
                continue;
            }
            let width = span.end.saturating_sub(span.start);
            if best.is_none_or(|(bs, bw, _)| (span.start, width) > (bs, bw)) {
                best = Some((span.start, width, *scope));
            }
        }
        let Some((_, _, scope)) = best else { return Vec::new() };
        let mut out = Vec::new();
        for scope in scopes.scope_chain(Some(scope)) {
            for entry in scopes.entries(scope) {
                out.push(entry.name().clone());
            }
        }
        out
    }

    /// Access the inference result, if available.
    pub(crate) fn infer(&self) -> Option<&InferenceResult> {
        match &self.body_or_sig {
            Some(BodyOrSig::Body { infer, .. }) => infer.as_ref(),
            None => None,
        }
    }

    /// Access the source map.
    pub(crate) fn source_map(&self) -> Option<&BodySourceMap> {
        match &self.body_or_sig {
            Some(BodyOrSig::Body { source_map, .. }) => Some(source_map),
            None => None,
        }
    }
}

impl SourceAnalyzer {
    /// Map a byte offset to the nearest ExprId in the body.
    pub(crate) fn expr_id(&self, offset: usize) -> Option<ExprId> {
        self.source_map()?.expr_at_offset(offset)
    }

    /// Map a byte offset to the nearest PatId in the body.
    pub(crate) fn pat_id(&self, offset: usize) -> Option<PatId> {
        self.source_map()?.pat_at_offset(offset)
    }

    /// Get the inferred type of an expression by ExprId.
    pub(crate) fn type_of_expr(&self, id: ExprId) -> Option<&Ty> {
        self.infer()?.expr_ty(id)
    }

    /// Get the inferred type of a pattern by PatId.
    pub(crate) fn type_of_pat(&self, id: PatId) -> Option<&Ty> {
        self.infer()?.pat_ty(id)
    }

    /// Resolve a path/name at the current scope, returning a typed `PathResolution`.
    /// Takes `db` as first parameter. Tries value
    /// namespace first, then type namespace.
    pub(crate) fn resolve_path(
        &self,
        db: &dyn salsa::Database,
        file_text: base_db::FileText,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let resolver = self.make_resolver();
        // — try value namespace first, then type namespace.
        if let Some(value_ns) = resolver.resolve_path_in_value_ns(db, name) {
            return Some(crate::PathResolution::from_value_ns(db, file_text, value_ns));
        }
        if let Some(type_ns) = resolver.resolve_path_in_type_ns(db, name) {
            return Some(crate::PathResolution::from_type_ns(db, file_text, type_ns));
        }
        // Check if it's a type variable ('n, 'm, etc.)
        if name.starts_with('\'') {
            return Some(crate::PathResolution::TypeVar(crate::TypeVar {
                name: hir_def::Name::new(name),
            }));
        }
        None
    }

    /// Returns the DefIdx of the struct field definition.
    pub(crate) fn resolve_field(&self, expr_id: ExprId) -> Option<hir_def::DefId> {
        self.infer()?.field_resolutions.get(&expr_id).copied()
    }

    /// Returns `(FunctionId, FileId)` identifying the resolved function.
    pub(crate) fn resolve_method_call(
        &self,
        expr_id: ExprId,
    ) -> Option<(hir_def::item_id::FunctionId, base_db::FileId)> {
        self.infer()?.method_resolutions.get(&expr_id).copied()
    }

    /// Tries field resolution first, then method resolution.
    pub(crate) fn resolve_field_or_method(
        &self,
        expr_id: ExprId,
    ) -> Option<Either<hir_def::DefId, (hir_def::item_id::FunctionId, base_db::FileId)>> {
        if let Some(field) = self.infer()?.field_resolutions.get(&expr_id) {
            return Some(Either::Left(*field));
        }
        if let Some(method) = self.infer()?.method_resolutions.get(&expr_id) {
            return Some(Either::Right(*method));
        }
        None
    }
}
