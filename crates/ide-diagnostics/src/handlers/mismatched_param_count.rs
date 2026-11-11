//! Handler for `AnyDiagnostic::MismatchedParamCount`.
//!
//! A function's parameter list against the `val` that declares it —
//! upstream reports this at the parameter list itself. `MismatchedArgCount`
//! is the same question asked at a *call*; nothing asked it at the
//! definition, because inference never read the declared signature.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, MismatchedParamCount};

// Diagnostic: mismatched-param-count
//
// This diagnostic is triggered when a function definition binds fewer
// parameters than its `val` specification declares. One parameter against
// several is not reported: it binds the whole argument tuple.
//
// ```sail
// val f : (int, int, int) -> int
// function f(x, y) = x
// ```
pub(crate) fn mismatched_param_count(
    _ctx: &DiagnosticsContext<'_>,
    d: &MismatchedParamCount,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("mismatched-param-count"),
        format!(
            "this definition takes {} parameter{}, but its `val` declares {}",
            d.found,
            if d.found == 1 { "" } else { "s" },
            d.expected,
        ),
        crate::node_file_range(&d.node),
    )
}
