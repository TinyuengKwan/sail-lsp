//! Handler for `AnyDiagnostic::UndeclaredEffect`.
//!
//! Error: function uses an effect not declared in its signature.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UndeclaredEffect};

// Diagnostic: undeclared-effect
//
// Not produced: nothing constructs `AnyDiagnostic::UndeclaredEffect` yet.
//
// This diagnostic would be triggered when a function performs an effect — a
// register write, say — that its `val` specification's effect set does not
// declare.
//
// ```sail
// register R : int
// val f : unit -> unit effect {rreg}
// function f() = R = 1
// ```
pub(crate) fn undeclared_effect(_ctx: &DiagnosticsContext<'_>, d: &UndeclaredEffect) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("undeclared-effect"),
        format!("function `{}` uses undeclared effect `{}`", d.name, d.effect),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UndeclaredEffect(UndeclaredEffect {
            name: "my_fn".to_string(),
            effect: "rreg".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        let msg = d.unwrap().message;
        assert!(msg.contains("my_fn"));
        assert!(msg.contains("rreg"));
    }
}
