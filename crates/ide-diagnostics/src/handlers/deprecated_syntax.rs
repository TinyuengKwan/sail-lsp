//! Handler for `AnyDiagnostic::DeprecatedSyntax`.
//!
//! Fix: remove deprecated syntax element.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DeprecatedSyntax, DiagnosticsContext};

pub(crate) fn deprecated_syntax(_ctx: &DiagnosticsContext<'_>, d: &DeprecatedSyntax) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("deprecated-syntax", Severity::Warning),
        &d.message,
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Fix: remove the deprecated syntax element.
///
/// Deletes the entire deprecated node (e.g., `effect {}` block).
fn fixes(d: &DeprecatedSyntax) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    let edit = TextEdit { range, new_text: String::new() };
    Some(vec![crate::fix(
        "remove_deprecated",
        "Remove deprecated syntax",
        SourceChange::from_text_edit(edit),
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::DeprecatedSyntax(DeprecatedSyntax {
            message: "effect block is deprecated".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("effect block is deprecated"));
    }
}
