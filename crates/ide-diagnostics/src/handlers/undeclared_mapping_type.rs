//! Handler for `AnyDiagnostic::UndeclaredMappingType`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, UndeclaredMappingType};

pub(crate) fn undeclared_mapping_type(
    _ctx: &DiagnosticsContext<'_>,
    d: &UndeclaredMappingType,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("undeclared-mapping-type"),
        format!("mapping `{}` does not have a declared type", d.name),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UndeclaredMappingType(UndeclaredMappingType {
            name: "my_mapping".to_string(),
            node: test_node(0, 10),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("my_mapping"));
    }
}
