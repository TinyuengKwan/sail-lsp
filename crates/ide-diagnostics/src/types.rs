//! Re-export diagnostic types from their canonical location in hir-def.
//!
//! The types were moved to `hir-def::diagnostics` so lower crates
//! (hir-ty) can use them without depending on ide-diagnostics.
//! All existing `ide_diagnostics::{Diagnostic, DiagnosticCode, Severity}`
//! paths continue to work through these re-exports.

// Diagnostic, Severity, and DiagnosticTag are now defined/re-exported
// from ide-diagnostics/src/lib.rs directly, matching RA's pattern.
pub use hir_def::diagnostics::DiagnosticCode;
