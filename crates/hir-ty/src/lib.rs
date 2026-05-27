//! Type-checker support layers.
//!
//! Owns the type representation (`Ty`, `TyKind`), type inference
//! (`infer/`), pattern exhaustiveness (`match_check`), and supporting
//! modules (display, overloading, etc.).
//!
//! # RA alignment (see DIFF_NOTES.md for full analysis)
//!
//! - `Ty` / `TyKind`: ALIGN wrapper shape; 11 vs RA's 30+ variants; no lifetime.
//! - `InferenceResult`: ALIGN ArenaMap fields; CUSTOM dual legacy/typed diagnostics.
//! - `InferenceContext`: ALIGN body/return_ty/diverges/table; CUSTOM is_pure_context/observed_effects.
//! - `HirDatabase`: ALIGN trait hierarchy; 5 queries vs RA's ~40+; manual trait (not macro).
//! - CUSTOM modules: z3_solver, overload, effects, nexp, existential, mapping, workspace, flow.

pub use parser::Span;

// Type representation at crate root
pub mod ty;
pub use ty::{
    CompareOp, ConstraintExpr, FnSig, InferTy, Kind, NumericExpr, Scalar, Ty, TyArg, TyCtor, TyKind,
};

pub mod cancel;
pub mod db;
pub mod diagnostics;
pub mod infer;
/// Type lowering (CST → Ty).
pub mod lower;
pub mod query;

pub mod display;
pub mod flow;
pub mod inhabitedness;
pub mod method_resolution;
pub mod nexp;
pub mod representability;
pub mod utils;

pub use cancel::CancellationToken;
pub use db::HirDatabase;
pub use display::{HirDisplay, HirDisplayWrapper, HirFormatter};
pub use infer::{
    check_file, check_file_with_records, check_file_with_workspace, diff_symbol_sig_hashes,
    infer_expr_type_text_in_files, CrossFileRecords, Diverges, InferenceDiagnostic,
    InferenceResult, Subst, TypeCheckResult, TypeMismatch, WorkspaceContext,
};
pub use nexp::{NConstraint, Nexp};
