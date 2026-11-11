//! Handler for `AnyDiagnostic::EffectViolation`.
//!
//! Sail-specific (no RA counterpart).
//!
//! No fix: `effect { ... }` annotations are deprecated syntax in upstream
//! Sail (`deprecated-effect-annotation`), so there is nothing to add.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, EffectViolation};

// Diagnostic: effect-violation
//
// Not produced: nothing constructs `is_declared_pure: true` yet, so no function
// body is checked in a pure context.
//
// This diagnostic would be triggered when a function declared pure reads or
// writes a register, throws, or calls a function that does. Upstream Sail
// dropped explicit effect annotations, and every type scheme here is built as
// not declared pure.
//
// ```sail
// register R : int
// val f : unit -> int
// function f() = R
// ```
/// Sail-specific: effect purity violation diagnostic.
pub(crate) fn effect_violation(_ctx: &DiagnosticsContext<'_>, d: &EffectViolation) -> Diagnostic {
    // Check if the violating effect has associated outcomes (FunctionEffects::outcomes).
    // Outcomes are string-typed side effects tracked separately from EffectTag.
    let effects = hir::effects::FunctionEffects::default();
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
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::EffectViolation(EffectViolation {
            effect: hir::EffectTag::Throw,
            context: "pure function",
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("Throw"));
    }
}

#[test]
fn offers_no_fix_because_effect_annotations_are_deprecated_syntax() {
    crate::tests::check_no_fix(
        "register R : int\nval f : int -> int effect pure\nfunction f(x) = { R = x; x }\n",
        "effect-violation",
    );
}
