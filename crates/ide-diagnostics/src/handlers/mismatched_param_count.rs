//! Handler for `AnyDiagnostic::MismatchedParamCount`.
//!
//! A function's parameter list against the `val` that declares it —
//! upstream reports this at the parameter list itself. `MismatchedArgCount`
//! is the same question asked at a *call*; nothing asked it at the
//! definition, because inference never read the declared signature.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, MismatchedParamCount};

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
