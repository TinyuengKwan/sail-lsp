//! Handler for `AnyDiagnostic::ShadowedBinding`.
//!
//! Warning: variable shadows an existing binding.
//!
//! Use semantic rename; changing only the declaration is unsafe.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, ShadowedBinding};

pub(crate) fn shadowed_binding(_ctx: &DiagnosticsContext<'_>, d: &ShadowedBinding) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("shadowed-binding", Severity::Warning),
        format!("variable `{}` shadows an existing binding", d.name),
        crate::node_file_range(&d.node),
    )
}

// Renaming a declaration alone can corrupt syntax or leave references bound
// to the wrong symbol. Use the semantic rename refactoring instead.

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::ShadowedBinding(ShadowedBinding {
            name: "x".to_string(),
            node: test_node(0, 1),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("shadows"));
    }
}
