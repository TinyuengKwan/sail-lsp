//! Handler for `AnyDiagnostic::RedundantTypeAnnotation`.
//!
//! Hint severity — informational.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, RedundantTypeAnnotation};

pub(crate) fn redundant_type_annotation(
    _ctx: &DiagnosticsContext<'_>,
    d: &RedundantTypeAnnotation,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("redundant-type-annotation", Severity::Warning),
        "type annotation is redundant",
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::RedundantTypeAnnotation(RedundantTypeAnnotation {
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("redundant"));
    }
}
