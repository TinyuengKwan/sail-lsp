//! HIR data types (shared between definition and type layers).
//!
//! `type_ref.rs`, expression/pattern enums, and other shared types.

pub mod type_ref;

// Re-export expr/pat types from expr_store::hir for backward compat.
// In RA, these live directly in `hir.rs` (same module). In sail-lsp
// they are in `expr_store/hir.rs` but accessible via `hir_def::hir::*`.
pub use crate::expr_store::hir::*;
