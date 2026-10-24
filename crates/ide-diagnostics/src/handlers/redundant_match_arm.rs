//! Handler for `AnyDiagnostic::RedundantMatchArm`.
//!
//! Diagnostic: redundant-match-arm
//!
//! An arm whose pattern an earlier arm already covers can never run.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, RedundantMatchArm};

pub(crate) fn redundant_match_arm(
    _ctx: &DiagnosticsContext<'_>,
    d: &RedundantMatchArm,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("redundant-match-arm", Severity::Warning),
        "unreachable match arm: an earlier arm already covers this pattern",
        crate::node_file_range(&d.node),
    )
    .with_unused(true)
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::RedundantMatchArm(RedundantMatchArm { node: test_node(0, 5) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unreachable match arm"));
    }
}
