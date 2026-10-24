//! Handler for `AnyDiagnostic::InvalidStringPattern`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;

use crate::{DiagnosticsContext, InvalidStringPattern};

pub(crate) fn invalid_string_pattern(
    ctx: &DiagnosticsContext<'_>,
    d: &InvalidStringPattern,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("invalid-string-pattern"),
        format!(
            "cannot match string-append pattern against non-string type `{}`",
            d.found.display_to_string(ctx.db())
        ),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::InvalidStringPattern(InvalidStringPattern {
            found: hir::Ty::scalar(hir::Scalar::Int),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot match string-append pattern"));
    }
}
