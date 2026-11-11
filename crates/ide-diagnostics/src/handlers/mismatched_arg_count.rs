//! Handler for `AnyDiagnostic::MismatchedArgCount`.
//!
//! No fix: which argument to add or remove is not known here, so there is
//! no edit that is right.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, MismatchedArgCount};

// Diagnostic: mismatched-arg-count
//
// This diagnostic is triggered when a call passes a different number of
// arguments than the callee's signature declares. An unfinished call — one
// whose `)` has not been typed yet — is not reported.
//
// ```sail
// val takes_two : (int, int) -> int
// function takes_two(x, y) = x
// function test() -> int = takes_two(1)
// ```
pub(crate) fn mismatched_arg_count(
    _ctx: &DiagnosticsContext<'_>,
    d: &MismatchedArgCount,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("mismatched-arg-count"),
        format!(
            "expected {} argument{}, found {}",
            d.expected,
            if d.expected == 1 { "" } else { "s" },
            d.found,
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
        let diag = AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
            expected: 2,
            found: 3,
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected 2 arguments, found 3"));
    }

    #[test]
    fn singular_argument() {
        let diag = AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
            expected: 1,
            found: 0,
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected 1 argument,"));
    }
}

#[test]
fn offers_no_fix_because_the_missing_argument_is_unknown() {
    crate::tests::check_no_fix(
        "val takes_two : (int, int) -> int\nfunction takes_two(x, y) = x\nfunction test() -> int = takes_two(1)\n",
        "mismatched-arg-count",
    );
}
