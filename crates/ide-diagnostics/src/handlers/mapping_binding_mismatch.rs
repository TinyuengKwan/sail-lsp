//! Handler for `AnyDiagnostic::MappingBindingMismatch`.
//!
//! Sail-specific: triggered when a bidirectional mapping clause has
//! bindings that don't appear on both sides.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, MappingBindingMismatch};

// Diagnostic: type-error
//
// Not produced: nothing constructs
// `InferenceDiagnostic::MappingBindingMismatch` yet.
//
// This diagnostic would be triggered when a bidirectional `mapping` clause
// binds a name on one side of `<->` and never mentions it on the other, so one
// direction of the mapping has no value for it. Such a clause is reported
// today as the `type-error` the unbound side causes.
//
// ```sail
// mapping m : bits(2) <-> bits(2) = { 0b0 @ x <-> 0b00 }
// ```
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
