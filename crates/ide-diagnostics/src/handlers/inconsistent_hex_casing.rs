//! Handler for `AnyDiagnostic::InconsistentHexCasing`.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};

use crate::{DiagnosticsContext, InconsistentHexCasing};

pub(crate) fn inconsistent_hex_casing(
    _ctx: &DiagnosticsContext<'_>,
    d: &InconsistentHexCasing,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("inconsistent-hex-casing", Severity::Warning),
        format!("inconsistent hexadecimal casing in `{}`", d.literal),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::InconsistentHexCasing(InconsistentHexCasing {
            literal: "0xaBcD".to_string(),
            node: test_node(0, 6),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("inconsistent hexadecimal casing"));
    }
}
