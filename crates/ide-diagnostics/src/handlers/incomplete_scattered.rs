//! Handler for `AnyDiagnostic::IncompleteScattered`.
//!
//! Sail-specific: scattered definitions require an `end` marker.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, IncompleteScattered};

pub(crate) fn incomplete_scattered(
    _ctx: &DiagnosticsContext<'_>,
    d: &IncompleteScattered,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("incomplete-scattered", Severity::Warning),
        format!("scattered {} `{}` missing `end` marker", d.kind, d.name),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::IncompleteScattered(IncompleteScattered {
            name: "my_union".to_string(),
            kind: "union".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("scattered union `my_union` missing `end` marker"));
    }
}
