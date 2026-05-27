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
use hir_def::bodies::CallableBody;
use hir_def::body::{Body, BodySourceMap, ExprId, PatId};
use hir_def::nameres::DefMap;
use hir_def::resolver::Resolver;
use hir_def::Span;
use hir_ty::infer::{InferenceResult, Ty};

use base_db::FileId;
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
    #[allow(dead_code)]
    pub(crate) file_id: FileId,
    /// Use `make_resolver()` to derive a borrowing Resolver on demand.
    pub(crate) resolver: Arc<DefMap>,
    /// Body/sig data (None if at module level).
    pub(crate) body_or_sig: Option<BodyOrSig>,
    /// Name of the containing callable, if inside a body.
    /// Stored during construction for `Semantics::containing_function`.
    pub(crate) containing_callable_name: Option<String>,
}

impl SourceAnalyzer {
    /// Create a SourceAnalyzer for a position inside a callable body,
    /// with type inference results available.
    #[allow(dead_code)]
    pub(crate) fn new_for_body(
        file_id: FileId,
        callable: &CallableBody,
        resolver: Arc<DefMap>,
        infer: Option<InferenceResult>,
    ) -> Self {
        Self {
            file_id,
            resolver,
            body_or_sig: Some(BodyOrSig::Body {
                body: (*callable.body).clone(),
                source_map: (*callable.source_map).clone(),
                infer,
            }),
            containing_callable_name: Some(callable.name.clone()),
        }
    }

    /// Create a SourceAnalyzer for a position inside a callable body,
    /// WITHOUT type inference results (faster, for navigation-only queries).
    #[allow(dead_code)]
    pub(crate) fn new_for_body_no_infer(
        file_id: FileId,
        callable: &CallableBody,
        resolver: Arc<DefMap>,
    ) -> Self {
        Self::new_for_body(file_id, callable, resolver, None)
    }

    /// Create a SourceAnalyzer at module level (no body context).
    /// Used when the cursor is outside any callable body.
    #[allow(dead_code)]
    pub(crate) fn new_for_resolver(file_id: FileId, resolver: Arc<DefMap>) -> Self {
        Self { file_id, resolver, body_or_sig: None, containing_callable_name: None }
    }

    /// Construct a borrowing `Resolver` from the stored `Arc<DefMap>`.
    pub(crate) fn make_resolver(&self) -> Resolver<'_> {
        Resolver::for_file(&self.resolver)
    }
}

impl SourceAnalyzer {
    /// Access the inference result, if available.
    pub(crate) fn infer(&self) -> Option<&InferenceResult> {
        match &self.body_or_sig {
            Some(BodyOrSig::Body { infer, .. }) => infer.as_ref(),
            None => None,
        }
    }

    /// Access the body (None if at module level).
    #[allow(dead_code)]
    pub(crate) fn body(&self) -> Option<&Body> {
        match &self.body_or_sig {
            Some(BodyOrSig::Body { body, .. }) => Some(body),
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

#[allow(dead_code)] // TODO: G-7 infrastructure, will be consumed by Semantics queries
impl SourceAnalyzer {
    /// Map a byte offset to the nearest ExprId in the body.
    pub(crate) fn expr_id(&self, offset: usize) -> Option<ExprId> {
        self.source_map()?.expr_at_offset(offset)
    }

    /// Map a span to an ExprId.
    pub(crate) fn expr_id_at_span(&self, span: Span) -> Option<ExprId> {
        self.source_map()?.expr_at_span(span)
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

    /// Get the inferred type at a byte offset (expression or pattern).
    /// Convenience method combining expr_id + type_of_expr,
    /// with pattern fallback.
    pub(crate) fn type_at_offset(&self, offset: usize) -> Option<&Ty> {
        if let Some(id) = self.expr_id(offset) {
            if let Some(ty) = self.type_of_expr(id) {
                return Some(ty);
            }
        }
        if let Some(id) = self.pat_id(offset) {
            if let Some(ty) = self.type_of_pat(id) {
                return Some(ty);
            }
        }
        None
    }

    /// Resolve a path/name at the current scope, returning a typed `PathResolution`.
    /// Takes `db` as first parameter. Tries value
    /// namespace first, then type namespace.
    pub(crate) fn resolve_path(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let resolver = self.make_resolver();
        // — try value namespace first, then type namespace.
        if let Some(value_ns) = resolver.resolve_path_in_value_ns(db, name) {
            return Some(crate::PathResolution::from_value_ns(value_ns));
        }
        if let Some(type_ns) = resolver.resolve_path_in_type_ns(db, name) {
            return Some(crate::PathResolution::from_type_ns(type_ns));
        }
        // Check if it's a type variable ('n, 'm, etc.)
        if name.starts_with('\'') {
            return Some(crate::PathResolution::TypeParam(crate::GenericParam {
                name: hir_def::Name::new(name),
            }));
        }
        None
    }

    /// Resolve a name in the type namespace only.
    pub(crate) fn resolve_path_in_type_ns(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        name: &str,
    ) -> Option<hir_def::resolver::TypeNs> {
        self.make_resolver().resolve_path_in_type_ns(db, name)
    }

    /// Resolve a name in the value namespace only.
    ///
    /// Takes `db` ().
    pub(crate) fn resolve_path_in_value_ns(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        name: &str,
    ) -> Option<hir_def::resolver::ValueNs> {
        self.make_resolver().resolve_path_in_value_ns(db, name)
    }

    /// Raw name resolution (lower-level, returns hir-def Resolution directly).
    /// Prefer `resolve_path()` for typed PathResolution.
    pub(crate) fn resolve_name_raw(&self, name: &str) -> hir_def::resolver::Resolution {
        self.make_resolver().resolve_name(name)
    }

    /// Returns the DefId of the struct field definition.
    pub(crate) fn resolve_field(
        &self,
        expr_id: ExprId,
    ) -> Option<hir_def::ModuleDefId> {
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
    ) -> Option<Either<hir_def::ModuleDefId, (hir_def::item_id::FunctionId, base_db::FileId)>> {
        if let Some(field) = self.infer()?.field_resolutions.get(&expr_id) {
            return Some(Either::Left(*field));
        }
        if let Some(method) = self.infer()?.method_resolutions.get(&expr_id) {
            return Some(Either::Right(*method));
        }
        None
    }

    /// Get the span for an ExprId (reverse lookup: HIR → source).
    pub(crate) fn expr_span(&self, id: ExprId) -> Option<Span> {
        self.source_map()?.expr_syntax(id)
    }

    /// Get the span for a PatId (reverse lookup: HIR → source).
    pub(crate) fn pat_span(&self, id: PatId) -> Option<Span> {
        self.source_map()?.pat_syntax(id)
    }
}
