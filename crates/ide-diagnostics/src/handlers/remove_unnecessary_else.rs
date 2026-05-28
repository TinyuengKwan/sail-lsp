//! Handler for `AnyDiagnostic::RemoveUnnecessaryElse`.
//! Emitted when an `if` expression's `then` branch diverges
//! (return/throw/exit), making the `else` branch unnecessary.
//! The code after the `if` can be written at the same level.
//!
//! Fix: remove the `else` block (unresolved — resolved via codeAction/resolve).

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, RemoveUnnecessaryElse};

/// (`remove_unnecessary_else.rs:21-41`).
pub(crate) fn remove_unnecessary_else(
    _ctx: &DiagnosticsContext<'_>,
    d: &RemoveUnnecessaryElse,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("redundant-type-annotation", Severity::Warning),
        "unnecessary `else` because `then` branch diverges".to_string(),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix stub: remove the unnecessary `else`.
///
/// that the client resolves via `codeAction/resolve`.
fn fixes(d: &RemoveUnnecessaryElse) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    Some(vec![crate::unresolved_fix("remove_unnecessary_else", "Remove unnecessary `else`", range)])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag =
            AnyDiagnostic::RemoveUnnecessaryElse(RemoveUnnecessaryElse { node: test_node(0, 4) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unnecessary `else`"));
    }
}
