//! Handler for `AnyDiagnostic::IncompleteScattered`.
//!
//! Sail-specific: scattered definitions require an `end` marker.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, IncompleteScattered};

pub(crate) fn incomplete_scattered(
    _ctx: &DiagnosticsContext<'_>,
    d: &IncompleteScattered,
) -> Diagnostic {
    use hir::scattered::ScatteredDiagnosticKind as K;
    let name = &d.name;
    // An orphan `end` is what upstream rejects, and it says so in these
    // words (`type_check.ml`); the other two it accepts, so they are hints.
    let (code, message) = match d.problem {
        K::MissingEnd => (
            DiagnosticCode::SailLint("incomplete-scattered", Severity::Hint),
            format!("scattered {} `{name}` is never ended", d.kind),
        ),
        K::NoClauses => (
            DiagnosticCode::SailLint("incomplete-scattered", Severity::Hint),
            format!("scattered {} `{name}` has no clauses", d.kind),
        ),
        K::OrphanEnd => (
            DiagnosticCode::SailError("incomplete-scattered"),
            format!("{name} is not a scattered definition, so it cannot be ended"),
        ),
    };
    Diagnostic::new(code, message, crate::node_file_range(&d.node))
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
            problem: hir::scattered::ScatteredDiagnosticKind::MissingEnd,
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("scattered union `my_union` is never ended"));
    }
}
