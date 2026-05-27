//! Handler for `AnyDiagnostic::NoOverloading`.
//!
//! Diagnostic: no-overloading
//!
//! Sail-specific: triggered when no overload candidate matches a
//! function call. Conceptually.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, NoOverloading};

pub(crate) fn no_overloading(_ctx: &DiagnosticsContext<'_>, d: &NoOverloading) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        format!("no matching overload for `{}`", d.name),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::NoOverloading(NoOverloading {
            name: "my_func".to_string(),
            node: test_node(0, 7),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("my_func"));
    }
}
