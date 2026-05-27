//! Handler for `AnyDiagnostic::MissingEffectAnnotation`.
//!
//! Warning: function has side effects but no effect annotation.
//!
//! Fix: add effect annotation to the function signature.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;

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
    .with_fixes(fixes(d))
}

/// Unresolved fix: add effect annotation to the val spec.
fn fixes(d: &MissingEffectAnnotation) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    Some(vec![crate::unresolved_fix(
        "add_effect_annotation",
        &format!("Add effect annotation `{}`", d.effects),
        range,
    )])
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
