//! Handler for `AnyDiagnostic::UnusedVariable`.
//! Emitted when a `let`/`var` binding or function parameter is never
//! referenced in its scope. Offers a quick-fix to prefix with `_`.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, UnusedVariable};

pub(crate) fn unused_variables(_ctx: &DiagnosticsContext<'_>, d: &UnusedVariable) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unused-variable", Severity::Warning),
        format!("unused variable `{}`", d.name),
        crate::node_file_range(&d.node),
    )
    .with_unused(true)
    .with_fixes(fixes(d))
}

/// Generate fix: prefix the variable name with `_`.
fn fixes(d: &UnusedVariable) -> Option<Vec<ide_db::assists::Assist>> {
    let new_name = format!("_{}", d.name);
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    let edit = TextEdit { range, new_text: new_name.clone() };
    let source_change = SourceChange::from_text_edit(edit);
    Some(vec![crate::fix(
        "unused_variables",
        &format!("Prefix with underscore: `{new_name}`"),
        source_change,
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnusedVariable(UnusedVariable {
            name: "x".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unused variable `x`"));
    }
}
