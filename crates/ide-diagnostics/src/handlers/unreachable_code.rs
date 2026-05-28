//! Handler for `AnyDiagnostic::UnreachableCode`.
//! Fix : remove unreachable code.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, UnreachableCode};

pub(crate) fn unreachable_code(_ctx: &DiagnosticsContext<'_>, d: &UnreachableCode) -> Diagnostic {
    let mut diag = Diagnostic::new(
        DiagnosticCode::SailLint("unreachable-code", Severity::Hint),
        "unreachable code",
        crate::node_file_range(&d.node),
    )
    .with_unused(true);
    diag.fixes = fixes(d);
    diag
}

/// Fix: delete the unreachable code block.
fn fixes(d: &UnreachableCode) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    let edit = TextEdit { range, new_text: String::new() };
    Some(vec![crate::fix(
        "remove_unreachable_code",
        "Remove unreachable code",
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
        let diag = AnyDiagnostic::UnreachableCode(UnreachableCode { node: test_node(0, 5) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unreachable code"));
    }
}
