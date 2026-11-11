//! Handler for `AnyDiagnostic::UnresolvedInclude`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UnresolvedInclude};

// Diagnostic: unresolved-include
//
// This diagnostic is triggered when an `$include` names a file that is not
// in the project: not on disk at the path written, and not a library header
// the configured Sail installation provides.
//
// ```sail
// $include "missing.sail"
// ```
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
