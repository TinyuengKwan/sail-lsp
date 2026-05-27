//! Handler for `AnyDiagnostic::UnresolvedQuants`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UnresolvedQuants};

pub(crate) fn unresolved_quants(_ctx: &DiagnosticsContext<'_>, d: &UnresolvedQuants) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-quants"),
        format!("could not resolve type variables for `{}`: {}", d.name, d.constraint),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedQuants(UnresolvedQuants {
            name: "foo".to_string(),
            constraint: "'n > 0".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("could not resolve type variables"));
    }
}
