//! Handler for `AnyDiagnostic::InvalidSliceAssign`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use hir_ty::display::HirDisplay;

use crate::{DiagnosticsContext, InvalidSliceAssign};

pub(crate) fn invalid_slice_assign(
    _ctx: &DiagnosticsContext<'_>,
    d: &InvalidSliceAssign,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("invalid-slice-assign"),
        format!("cannot assign slice of non-vector type `{}`", d.found.display_to_string()),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::InvalidSliceAssign(InvalidSliceAssign {
            found: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot assign slice"));
    }
}
