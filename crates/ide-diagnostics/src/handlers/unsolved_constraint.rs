//! Handler for `AnyDiagnostic::UnsolvedConstraint`.
//!
//! Sail-specific (Z3 constraint solving).

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, UnsolvedConstraint};

// Diagnostic: unsolved-constraint
//
// This diagnostic is triggered when a type-level constraint — a bound on a
// width or an integer type variable — cannot be shown to hold at a call or a
// binding. When the solver could not decide the constraint rather than refute
// it, the finding says so and the server marks the analysis as unknown.
//
// ```sail
// val q : forall 'n, 'n >= 0. int('n) -> bits('n)
// function f(n : int) -> unit = { let _ = q(n); () }
// ```
/// Sail-specific: unsolved numeric constraint diagnostic.
pub(crate) fn unsolved_constraint(
    _ctx: &DiagnosticsContext<'_>,
    d: &UnsolvedConstraint,
) -> Diagnostic {
    let message = if d.proved_false {
        format!("constraint cannot hold: {}", d.constraint)
    } else {
        format!(
            "unsolved constraint: {} ({})",
            d.constraint,
            d.reason.unwrap_or("the analyzer could not establish this constraint")
        )
    };
    // Where the constraint came from. The resolver records the argument spans
    // that produced it, and knowing which argument is at fault is most of the
    // value in a constraint error — so each one is a *place*, which is what
    // `relatedInformation` is for. It used to be appended to the message as
    // `constraint from 110..111`, two integers the reader cannot act on.
    let file = crate::node_file_range(&d.node).file_id;
    let related = d
        .derived_from
        .iter()
        .map(|span| {
            (
                hir::FileRange { file_id: file, range: base_db::text_range(span.start, span.end) },
                "constraint from here".to_string(),
            )
        })
        .collect();
    let mut diagnostic = Diagnostic::new(
        DiagnosticCode::SailLint(
            "unsolved-constraint",
            if d.proved_false { Severity::Error } else { Severity::Warning },
        ),
        message,
        crate::node_file_range(&d.node),
    )
    .with_related(related);
    if !d.proved_false {
        diagnostic.unknown_reason = Some(d.reason_code.unwrap_or("not_established"));
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
            constraint: "'n >= 0".to_string(),
            reason: Some("the analyzer's constraint solving budget was exhausted"),
            reason_code: Some("budget_exhausted"),
            proved_false: false,
            derived_from: Vec::new(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        let d = d.unwrap();
        assert!(d.message.contains("unsolved constraint"));
        assert!(d.message.contains("budget was exhausted"));
        assert_eq!(d.unknown_reason, Some("budget_exhausted"));
        assert_eq!(d.severity, hir::diagnostics::Severity::Warning);
    }

    /// A constraint shown to have no solution is an error, and says so.
    #[test]
    fn proved_false_is_an_error() {
        let diag = AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
            constraint: "'m == 8 * 'n".to_string(),
            reason: None,
            reason_code: None,
            proved_false: true,
            derived_from: Vec::new(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag).expect("dispatches");
        assert!(d.message.contains("cannot hold"));
        assert_eq!(d.severity, hir::diagnostics::Severity::Error);
        assert_eq!(d.unknown_reason, None);
    }
}
