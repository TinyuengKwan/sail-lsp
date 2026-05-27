//! Handler for `AnyDiagnostic::EffectMismatchInOverride`.
//!
//! Error: mapping forward/backward clauses have different effects.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, EffectMismatchInOverride};

pub(crate) fn effect_mismatch_in_override(
    _ctx: &DiagnosticsContext<'_>,
    d: &EffectMismatchInOverride,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("effect-mismatch"),
        format!("effect mismatch in `{}`: expected ({}), found ({})", d.name, d.expected, d.found),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::EffectMismatchInOverride(EffectMismatchInOverride {
            name: "my_mapping".to_string(),
            expected: "pure".to_string(),
            found: "rreg".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("effect mismatch in `my_mapping`"));
    }
}
