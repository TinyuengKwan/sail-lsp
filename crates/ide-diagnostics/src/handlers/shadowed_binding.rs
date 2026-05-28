//! Handler for `AnyDiagnostic::ShadowedBinding`.
//!
//! Warning: variable shadows an existing binding.
//!
//! Fix : rename shadowed variable with `_` prefix.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, ShadowedBinding};

pub(crate) fn shadowed_binding(_ctx: &DiagnosticsContext<'_>, d: &ShadowedBinding) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("shadowed-binding", Severity::Warning),
        format!("variable `{}` shadows an existing binding", d.name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Fix: rename the shadowing variable with a `_` prefix.
fn fixes(d: &ShadowedBinding) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    let new_name = format!("_{}", d.name);
    let edit = TextEdit { range, new_text: new_name.clone() };
    Some(vec![crate::fix(
        "rename_shadowed",
        &format!("Rename to `{new_name}`"),
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
        let diag = AnyDiagnostic::ShadowedBinding(ShadowedBinding {
            name: "x".to_string(),
            node: test_node(0, 1),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("shadows"));
    }
}
