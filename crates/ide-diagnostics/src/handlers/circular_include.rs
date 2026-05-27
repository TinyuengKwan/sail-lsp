//! Handler for `AnyDiagnostic::CircularInclude`.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{CircularInclude, DiagnosticsContext};

pub(crate) fn circular_include(_ctx: &DiagnosticsContext<'_>, d: &CircularInclude) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("circular-include"),
        format!("circular $include detected: {}", d.cycle_description),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::CircularInclude(CircularInclude {
            cycle_description: "a.sail -> b.sail -> a.sail".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("a.sail -> b.sail -> a.sail"));
    }
}
