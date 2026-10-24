//! Handler for `AnyDiagnostic::DuplicateBinding`.
//!
//! Triggered when the same name appears twice in a single pattern tree.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, DuplicateBindingDiag};

pub(crate) fn duplicate_binding(
    _ctx: &DiagnosticsContext<'_>,
    d: &DuplicateBindingDiag,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("duplicate-definition"),
        // `Duplicate binding for {id} in pattern` (`type_check.ml:1650`),
        // with the earlier binding as a second caret. A struct pattern is
        // a different sentence upstream — the duplicate is a *field* there,
        // and `check_duplicate_fields` says so (`initial_check.ml:1360`).
        if d.in_struct_pattern {
            format!("Duplicate field '{}' in struct pattern", d.name)
        } else {
            format!("Duplicate binding for {} in pattern", d.name)
        },
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
            in_struct_pattern: false,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("Duplicate binding for x in pattern"));
    }

    /// The same duplicate in a struct pattern is a *field* upstream, and a
    /// different sentence: `Duplicate field 'f1' in struct pattern`
    /// (`initial_check.ml:1360`).
    #[test]
    fn a_struct_pattern_field_is_a_field() {
        let diag = AnyDiagnostic::DuplicateBinding(DuplicateBindingDiag {
            name: "f1".to_string(),
            node: test_node(0, 5),
            in_struct_pattern: true,
        });
        assert_eq!(
            check_dispatch(&diag).map(|d| d.message),
            Some("Duplicate field 'f1' in struct pattern".to_string())
        );
    }
}
