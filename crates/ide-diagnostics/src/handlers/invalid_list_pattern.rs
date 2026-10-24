//! Handler for `AnyDiagnostic::InvalidListPattern`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;

use crate::{DiagnosticsContext, InvalidListPattern};

pub(crate) fn invalid_list_pattern(
    ctx: &DiagnosticsContext<'_>,
    d: &InvalidListPattern,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("invalid-list-pattern"),
        format!(
            "cannot match cons pattern against non-list type `{}`",
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
        let diag = AnyDiagnostic::InvalidListPattern(InvalidListPattern {
            found: hir::Ty::scalar(hir::Scalar::Int),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot match cons pattern"));
    }
}
