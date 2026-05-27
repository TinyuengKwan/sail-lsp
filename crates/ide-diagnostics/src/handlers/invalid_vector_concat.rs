//! Handler for `AnyDiagnostic::InvalidVectorConcat`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, InvalidVectorConcat};

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
