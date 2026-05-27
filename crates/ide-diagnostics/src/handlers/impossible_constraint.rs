//! Handler for `AnyDiagnostic::ImpossibleConstraint`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, ImpossibleConstraint};

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
