//! Handler for `AnyDiagnostic::ReturnOutsideFunction`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, ReturnOutsideFunction};

// Diagnostic: return-outside-function
//
// Not produced: nothing constructs `AnyDiagnostic::ReturnOutsideFunction` yet.
//
// This diagnostic would be triggered when `return` appears where there is no
// function to return from, such as a top-level `let`.
//
// ```sail
// let x = return 1
// ```
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
