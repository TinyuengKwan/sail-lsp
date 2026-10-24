//! Handler for `AnyDiagnostic::InvalidSliceAssign`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;

use crate::{DiagnosticsContext, InvalidSliceAssign};

pub(crate) fn invalid_slice_assign(
    ctx: &DiagnosticsContext<'_>,
    d: &InvalidSliceAssign,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("invalid-slice-assign"),
        format!("cannot assign slice of non-vector type `{}`", d.found.display_to_string(ctx.db())),
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
            found: hir::Ty::scalar(hir::Scalar::Bool),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot assign slice"));
    }
}
