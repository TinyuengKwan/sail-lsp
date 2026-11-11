//! Handler for `AnyDiagnostic::UnresolvedIdent`.
//!
//! No fix: nothing here computes a candidate name, and a fix with no
//! edit is a label the editor cannot apply.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UnresolvedIdent};

// Diagnostic: unresolved-ident
//
// This diagnostic is triggered when a name is used in an expression and nothing
// in scope — no binding, parameter, register, `let`, enum member or declaration
// in the project — declares it.
//
// ```sail
// val g : int -> int
// function f() -> int = g(zzz)
// ```
/// `fn(ctx, d: &hir::UnresolvedIdent) -> Diagnostic`
pub(crate) fn unresolved_ident(_ctx: &DiagnosticsContext<'_>, d: &UnresolvedIdent) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-ident"),
        format!("unresolved identifier `{}`", d.name),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
            name: "foo".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unresolved identifier `foo`"));
    }
}

#[test]
fn offers_no_fix_without_a_candidate_name() {
    crate::tests::check_no_fix("function f() -> int = absent\n", "unresolved-ident");
}
