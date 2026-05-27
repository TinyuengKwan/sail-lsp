//! Handler for `AnyDiagnostic::UnresolvedInclude`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UnresolvedInclude};

pub(crate) fn unresolved_include(
    _ctx: &DiagnosticsContext<'_>,
    d: &UnresolvedInclude,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-include"),
        format!("unresolved $include `{}`", d.path),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedInclude(UnresolvedInclude {
            path: "missing.sail".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("missing.sail"));
    }
}
