//! Handler for `AnyDiagnostic::NotAFunction`.
//!
//! Diagnostic: type-error
//!
//! `let x : int = 3` then `x(1)`. The name is bound, and to something that
//! is not callable — upstream looks the callee up in the function namespace
//! alone and says "Function x not found. Did you mean f?".
//!
//! Distinct from `no-overloading`, which is about a name that denotes
//! nothing at all: saying "no matching overload for `x`" about a local
//! integer describes the wrong problem.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, NotAFunction};

pub(crate) fn not_a_function(_ctx: &DiagnosticsContext<'_>, d: &NotAFunction) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        format!("`{}` is not a function", d.name),
        crate::node_file_range(&d.node),
    )
}
