//! Handler for `AnyDiagnostic::DuplicateBinding`.
//!
//! Triggered when the same name appears twice in a single pattern tree.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, DuplicateBindingDiag};

pub(crate) fn duplicate_binding(
    _ctx: &DiagnosticsContext<'_>,
    d: &DuplicateBindingDiag,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("duplicate-definition"),
        format!("duplicate binding for `{}` in pattern", d.name),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::DuplicateBinding(DuplicateBindingDiag {
            name: "x".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("duplicate binding for `x`"));
    }
}
