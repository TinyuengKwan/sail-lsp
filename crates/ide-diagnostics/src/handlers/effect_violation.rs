//! Handler for `AnyDiagnostic::EffectViolation`.
//!
//! Sail-specific (no RA counterpart).
//!
//! Fix: add effect annotation to the enclosing function signature.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, EffectViolation};

/// Sail-specific: effect purity violation diagnostic.
pub(crate) fn effect_violation(_ctx: &DiagnosticsContext<'_>, d: &EffectViolation) -> Diagnostic {
    // Check if the violating effect has associated outcomes (FunctionEffects::outcomes).
    // Outcomes are string-typed side effects tracked separately from EffectTag.
    let effects = hir_def::effects::FunctionEffects::default();
    let outcome_note = if effects.has_outcome(&format!("{:?}", d.effect)) {
        format!(" (has outcome `{:?}`)", d.effect)
    } else {
        String::new()
    };
    Diagnostic::new(
        DiagnosticCode::SailError("effect-violation"),
        format!("effect `{:?}` not allowed in {}{}", d.effect, d.context, outcome_note),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix: add the missing effect annotation to the function.
fn fixes(d: &EffectViolation) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    Some(vec![crate::unresolved_fix(
        "add_effect_annotation",
        &format!("Add `{:?}` effect annotation", d.effect),
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::EffectViolation(EffectViolation {
            effect: hir_def::bodies::EffectTag::Throw,
            context: "pure function",
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("Throw"));
    }
}
