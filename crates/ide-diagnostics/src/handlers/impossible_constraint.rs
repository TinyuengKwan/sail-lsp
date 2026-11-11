//! Handler for `AnyDiagnostic::ImpossibleConstraint`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, ImpossibleConstraint};

// Diagnostic: impossible-constraint
//
// Not produced: nothing constructs `AnyDiagnostic::ImpossibleConstraint` yet; a
// constraint the solver refutes is reported as `unsolved-constraint`.
//
// This diagnostic would be triggered when a declaration's constraints can never
// all hold, so no value has the type it describes.
//
// ```sail
// val f : forall 'n, 'n > 0 & 'n < 0. int('n) -> unit
// ```
pub(crate) fn impossible_constraint(
    _ctx: &DiagnosticsContext<'_>,
    d: &ImpossibleConstraint,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("impossible-constraint"),
        format!("impossible constraint: {}", d.constraint),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::ImpossibleConstraint(ImpossibleConstraint {
            constraint: "32 == 64".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("impossible constraint: 32 == 64"));
    }
}
