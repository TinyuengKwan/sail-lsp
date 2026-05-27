//! Handler for `AnyDiagnostic::UnresolvedField`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use hir_ty::display::HirDisplay;

use crate::{DiagnosticsContext, UnresolvedField};

pub(crate) fn unresolved_field(_ctx: &DiagnosticsContext<'_>, d: &UnresolvedField) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-field"),
        format!("no field `{}` on type `{}`", d.name, d.receiver.display_to_string()),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedField(UnresolvedField {
            receiver: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Int),
            name: "bar".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("no field `bar`"));
    }
}
