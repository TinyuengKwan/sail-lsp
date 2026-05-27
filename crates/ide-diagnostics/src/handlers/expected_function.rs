//! Handler for `AnyDiagnostic::ExpectedFunction`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use hir_ty::display::HirDisplay;

use crate::{DiagnosticsContext, ExpectedFunction};

pub(crate) fn expected_function(_ctx: &DiagnosticsContext<'_>, d: &ExpectedFunction) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("expected-function"),
        format!("expected function, found `{}`", d.found.display_to_string()),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::ExpectedFunction(ExpectedFunction {
            found: hir_ty::infer::Ty::error(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected function"));
    }
}
