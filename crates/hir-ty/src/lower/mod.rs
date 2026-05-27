//! Type lowering: CST → Ty conversion.
//! Converts syntactic type representations (CST nodes, text strings)
//! into semantic `Ty` values used during type inference. This module
//! owns the "type reference → `Ty`" path, analogous to RA's
//! `TyLoweringContext::lower_ty`.
//!
//! # Entry points
//!
//! - `TyLoweringContext::lower_ty_from_text(text)` — parse text into Ty
//! - `TyLoweringContext::lower_ty_from_cst(node)` — lower CST node into Ty
//! - `TyLoweringContext::lower_scheme_from_cst(node)` — lower val spec into TypeScheme
//! - Free functions `lower_ty_from_text`, `lower_ty_from_cst` — for backward compat

pub mod diagnostics;

use crate::infer::{type_from_cst_node, type_from_type_text};
use crate::ty::Ty;

use self::diagnostics::TyLoweringDiagnostic;
pub use hir_def::type_ref::TypeRefId;

/// Context for lowering type references to `Ty` values.
/// ```ignore
/// pub struct TyLoweringContext<'db, 'a> {
///     pub db: &'db dyn HirDatabase,
///     resolver: &'a Resolver<'db>,
///     store: &'a ExpressionStore,
///     // ...
/// }
/// ```
///
/// - `'db`: database lifetime (salsa queries, interned types)
/// - `'a`: resolver / store borrows (can be shorter than `'db`)
pub struct TyLoweringContext<'db, 'a> {
    /// Database for salsa queries.
    pub db: &'db dyn salsa::Database,
    /// Resolver for name resolution during type lowering.
    pub resolver: Option<&'a hir_def::Resolver<'db>>,
    /// The callable/type definition whose signature is being lowered.
    pub owner: Option<&'a str>,
    /// Diagnostics accumulated during lowering.
    pub(crate) diagnostics: Vec<TyLoweringDiagnostic>,
}

impl<'db, 'a> TyLoweringContext<'db, 'a> {
    /// Create a new context with database and resolver.
    pub fn new(db: &'db dyn salsa::Database, resolver: &'a hir_def::Resolver<'db>) -> Self {
        Self { db, resolver: Some(resolver), owner: None, diagnostics: Vec::new() }
    }

    /// Create a context with only a database (no resolver).
    pub fn new_without_resolver(db: &'db dyn salsa::Database) -> Self {
        Self { db, resolver: None, owner: None, diagnostics: Vec::new() }
    }

    /// Set the owner (callable name) for this lowering context.
    pub fn with_owner(mut self, owner: &'a str) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Return diagnostics accumulated during type lowering.
    pub fn diagnostics(&self) -> &[TyLoweringDiagnostic] {
        &self.diagnostics
    }

    /// Lower a textual type annotation into a `Ty`.
    pub fn lower_ty_from_text(&self, text: &str) -> Ty {
        type_from_type_text(text)
    }

    /// Lower a CST type node into a `Ty`.
    pub fn lower_ty_from_cst(&self, node: &syntax::SyntaxNode) -> Ty {
        type_from_cst_node(node)
    }

    /// Lower a `TypeRef` into a `Ty`.
    ///
    /// This is the primary entry point — takes a structured TypeRef
    /// and produces a semantic Ty.
    pub fn lower_ty(&self, type_ref: &hir_def::hir::type_ref::TypeRef) -> Ty {
        use hir_def::hir::type_ref::{TypeArg, TypeRef};

        match type_ref {
            TypeRef::Named(name) => self.lower_ty_from_text(name),
            TypeRef::Var(name) => Ty::param(name.clone()),
            TypeRef::App { name, args } => {
                // Reconstruct text for now — future: direct TypeRef → Ty
                let args_text: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        TypeArg::Type(t) => format!("{}", t),
                        TypeArg::Value(v) => v.clone(),
                    })
                    .collect();
                let text = format!("{}({})", name, args_text.join(", "));
                self.lower_ty_from_text(&text)
            }
            TypeRef::Tuple(items) => {
                let tys: Vec<Ty> = items.iter().map(|t| self.lower_ty(t)).collect();
                Ty::tuple(tys)
            }
            TypeRef::Fn { params, ret } => {
                let param_tys: Vec<Ty> = params.iter().map(|t| self.lower_ty(t)).collect();
                let ret_ty = self.lower_ty(ret);
                Ty::function(param_tys, ret_ty)
            }
            TypeRef::Bidir { lhs, rhs } => {
                let lhs_ty = self.lower_ty(lhs);
                let rhs_ty = self.lower_ty(rhs);
                Ty::bidir(lhs_ty, rhs_ty)
            }
            TypeRef::Exist { vars, constraint: _, inner } => {
                let inner_ty = self.lower_ty(inner);
                Ty::exist(vars.clone(), crate::ty::ConstraintExpr::Bool(true), inner_ty)
            }
            TypeRef::Forall { vars: _, inner } => self.lower_ty(inner),
            TypeRef::Error => Ty::error(),
        }
    }
}

/// Lower a textual type annotation into a `Ty`.
///
/// Convenience wrapper — calls `type_from_type_text` directly.
/// New code should prefer `TyLoweringContext::lower_ty_from_text`.
pub fn lower_ty_from_text(text: &str) -> Ty {
    type_from_type_text(text)
}

/// Lower a CST type node into a `Ty`.
///
/// Convenience wrapper — calls `type_from_cst_node` directly.
/// New code should prefer `TyLoweringContext::lower_ty_from_cst`.
pub fn lower_ty_from_cst(node: &syntax::SyntaxNode) -> Ty {
    type_from_cst_node(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::TyKind;

    #[test]
    fn lower_simple_named_type() {
        let ty = lower_ty_from_text("int");
        assert!(matches!(ty.kind(), TyKind::Scalar(crate::ty::Scalar::Int)));
    }

    #[test]
    fn lower_bits_app_type() {
        let ty = lower_ty_from_text("bits(32)");
        match ty.kind() {
            TyKind::App { name, args, .. } => {
                assert_eq!(name, "bits");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected App, got {:?}", other),
        }
    }

    #[test]
    fn lower_tuple_type() {
        let ty = lower_ty_from_text("(int, bool)");
        assert!(matches!(ty.kind(), TyKind::Tuple(items) if items.len() == 2));
    }

    #[test]
    fn lower_function_type() {
        let ty = lower_ty_from_text("(int, int) -> bool");
        match ty.kind() {
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                assert_eq!(params.len(), 2);
                assert!(matches!(ret.kind(), TyKind::Scalar(crate::ty::Scalar::Bool)));
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn lower_error_for_empty() {
        let ty = lower_ty_from_text("");
        assert!(ty.is_error());
    }

    #[test]
    fn ty_lowering_context_diagnostics_starts_empty() {
        #[salsa::db]
        #[derive(Default, Clone)]
        struct TestDb {
            storage: salsa::Storage<Self>,
        }
        #[salsa::db]
        impl salsa::Database for TestDb {}

        let db = TestDb::default();
        let ctx = TyLoweringContext::new_without_resolver(&db);
        assert!(ctx.diagnostics().is_empty());
    }
}
