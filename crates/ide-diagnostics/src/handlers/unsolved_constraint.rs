//! Handler for `AnyDiagnostic::UnsolvedConstraint`.
//!
//! Sail-specific (Z3 constraint solving).

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, UnsolvedConstraint};

/// Sail-specific: unsolved numeric constraint diagnostic.
pub(crate) fn unsolved_constraint(
    _ctx: &DiagnosticsContext<'_>,
    d: &UnsolvedConstraint,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unsolved-constraint", Severity::Warning),
        format!("unsolved constraint: {}", d.constraint),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
            constraint: "'n >= 0".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unsolved constraint"));
    }
}
