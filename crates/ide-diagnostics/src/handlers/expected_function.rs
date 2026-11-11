//! Handler for `AnyDiagnostic::ExpectedFunction`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;

use crate::{DiagnosticsContext, ExpectedFunction};

// Diagnostic: expected-function
//
// Not produced: the parser forms a call only when the callee is a name —
// `f(1)(2)` parses as the call `f(1)` followed by a stray `(2)` — so the
// inference branch that pushes `InferenceDiagnostic::ExpectedFunction` for a
// non-name callee is one no program reaches.
//
// This diagnostic would be triggered when a call's callee is an expression
// that is not a name — the result of another call, say — so there is no
// signature to check the arguments against. A name bound to a non-function is
// `type-error`; a `function` defined against a mapping's `val` is
// `not-a-function-type`, from the item walker.
//
// ```sail
// val f : int -> int
// function g() -> int = f(1)(2)
// ```
pub(crate) fn expected_function(ctx: &DiagnosticsContext<'_>, d: &ExpectedFunction) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("expected-function"),
        format!("expected function, found `{}`", d.found.display_to_string(ctx.db())),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::ExpectedFunction(ExpectedFunction {
            found: hir::Ty::error(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected function"));
    }
}
