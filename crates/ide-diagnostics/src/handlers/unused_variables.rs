//! Handler for `AnyDiagnostic::UnusedVariable`.
//! Emitted when a `let`/`var` binding or function parameter is never
//! referenced in its scope. The `_` prefix fix is
//! `ide_assists::quick_fix_for_diagnostic`'s, so it is not built twice.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, UnusedVariable};

pub(crate) fn unused_variables(_ctx: &DiagnosticsContext<'_>, d: &UnusedVariable) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unused-variable", Severity::Warning),
        format!("unused variable `{}`", d.name),
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
        let diag = AnyDiagnostic::UnusedVariable(UnusedVariable {
            name: "x".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unused variable `x`"));
    }
}
