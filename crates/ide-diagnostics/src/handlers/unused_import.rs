//! Handler for `AnyDiagnostic::UnusedImport`.
//!
//! Warning: $include brings in a file from which no symbols are used.
//!
//! Fix : remove the $include line.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, UnusedImport};

pub(crate) fn unused_import(_ctx: &DiagnosticsContext<'_>, d: &UnusedImport) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unused-import", Severity::Warning),
        format!("unused $include \"{}\"", d.path),
        crate::node_file_range(&d.node),
    )
    .with_unused(true)
    .with_fixes(fixes(d))
}

/// Fix: remove the entire $include line.
fn fixes(d: &UnusedImport) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    let edit = TextEdit { range, new_text: String::new() };
    Some(vec![crate::fix(
        "remove_unused_import",
        &format!("Remove unused `$include \"{}\"`", d.path),
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
        let diag = AnyDiagnostic::UnusedImport(UnusedImport {
            path: "prelude.sail".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unused $include"));
    }
}
