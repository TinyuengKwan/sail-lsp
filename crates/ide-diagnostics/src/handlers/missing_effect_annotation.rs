//! Handler for `AnyDiagnostic::MissingEffectAnnotation`.
//!
//! Warning: function has side effects but no effect annotation.
//!
//! No fix: `effect { ... }` annotations are deprecated syntax in upstream
//! Sail (`deprecated-effect-annotation`), so there is nothing to add.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, MissingEffectAnnotation};

pub(crate) fn missing_effect_annotation(
    _ctx: &DiagnosticsContext<'_>,
    d: &MissingEffectAnnotation,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("missing-effect-annotation", Severity::Warning),
        format!("function `{}` has effects ({}) but no effect annotation", d.name, d.effects),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::MissingEffectAnnotation(MissingEffectAnnotation {
            name: "my_func".to_string(),
            effects: "rreg, wreg".to_string(),
            node: test_node(0, 7),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("function `my_func` has effects"));
    }
}
