//! Handler for `AnyDiagnostic::IncompleteMatch`.
//!
//! Diagnostic: missing-match-arm
//!
//! This diagnostic is triggered if `match` block is missing one or more
//! match arms.
//!
//! No fix here: the arms go before the `match`'s closing brace, which this
//! node's range does not say. `ide_assists::quick_fix_for_diagnostic` adds
//! the wildcard arm and the `add_missing_match_arms` assist the variants.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, IncompleteMatch};

// Diagnostic: missing-match-arm
//
// This diagnostic is triggered if `match` block is missing one or more match arms.
pub(crate) fn missing_match_arms(_ctx: &DiagnosticsContext<'_>, d: &IncompleteMatch) -> Diagnostic {
    let message = if d.missing_arms.is_empty() {
        "non-exhaustive match".to_string()
    } else {
        format!("non-exhaustive match: missing {}", d.missing_arms.join(", "))
    };
    Diagnostic::new(
        DiagnosticCode::SailLint("incomplete-match", Severity::Warning),
        message,
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::IncompleteMatch(IncompleteMatch {
            missing_arms: vec!["Foo".to_string(), "Bar".to_string()],
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("non-exhaustive match: missing Foo, Bar"));
    }

    #[test]
    fn empty_arms() {
        let diag = AnyDiagnostic::IncompleteMatch(IncompleteMatch {
            missing_arms: vec![],
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert_eq!(d.unwrap().message, "non-exhaustive match");
    }
}
