//! Handler for `AnyDiagnostic::InvalidVectorConcat`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, InvalidVectorConcat};

// Diagnostic: invalid-vector-concat
//
// Not produced: nothing constructs `AnyDiagnostic::InvalidVectorConcat` yet; a
// non-vector operand of `@` is reported as `type-error` by the concatenation
// check.
//
// This diagnostic would be triggered when a vector concatenation `@` is applied
// to something that is not a vector.
//
// ```sail
// function f() -> bits(8) = 0x0 @ true
// ```
pub(crate) fn invalid_vector_concat(
    _ctx: &DiagnosticsContext<'_>,
    d: &InvalidVectorConcat,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("invalid-vector-concat"),
        &d.message,
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::InvalidVectorConcat(InvalidVectorConcat {
            message: "cannot concatenate vectors of different types".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot concatenate vectors"));
    }
}
