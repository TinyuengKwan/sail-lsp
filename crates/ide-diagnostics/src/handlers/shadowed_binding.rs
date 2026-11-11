//! Handler for `AnyDiagnostic::ShadowedBinding`.
//!
//! Warning: variable shadows an existing binding.
//!
//! Use semantic rename; changing only the declaration is unsafe.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, ShadowedBinding};

// Diagnostic: shadowed-binding
//
// Not produced: nothing constructs `AnyDiagnostic::ShadowedBinding` yet.
//
// This diagnostic would be triggered when a `let` binds a name an enclosing
// scope already binds, hiding the outer binding for the rest of the block.
//
// ```sail
// function f(x : int) -> int = { let x = 2; x }
// ```
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
