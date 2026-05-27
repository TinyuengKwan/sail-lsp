//! Handler for `AnyDiagnostic::MappingBindingMismatch`.
//!
//! Diagnostic: mapping-binding-mismatch
//!
//! Sail-specific: triggered when a bidirectional mapping clause has
//! bindings that don't appear on both sides.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, MappingBindingMismatch};

pub(crate) fn mapping_binding_mismatch(
    _ctx: &DiagnosticsContext<'_>,
    d: &MappingBindingMismatch,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        format!(
            "identifier `{}` found on {} side of mapping, but not on the other",
            d.name, d.side
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
        let diag = AnyDiagnostic::MappingBindingMismatch(MappingBindingMismatch {
            name: "x".to_string(),
            side: "left".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("identifier `x`"));
    }
}
