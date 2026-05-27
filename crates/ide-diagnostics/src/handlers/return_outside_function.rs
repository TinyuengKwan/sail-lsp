//! Handler for `AnyDiagnostic::ReturnOutsideFunction`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, ReturnOutsideFunction};

pub(crate) fn return_outside_function(
    _ctx: &DiagnosticsContext<'_>,
    d: &ReturnOutsideFunction,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("return-outside-function"),
        "cannot use `return` outside a function body",
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag =
            AnyDiagnostic::ReturnOutsideFunction(ReturnOutsideFunction { node: test_node(0, 6) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("outside a function"));
    }
}
