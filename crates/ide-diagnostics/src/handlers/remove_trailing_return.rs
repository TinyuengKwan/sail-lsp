//! Handler for `AnyDiagnostic::RemoveTrailingReturn`.
//! Emitted when a function body ends with an explicit `return expr`
//! that is redundant because Sail uses implicit return for the last
//! expression.
//!
//! Fix: remove the `return` keyword (unresolved — resolved via codeAction/resolve).

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, RemoveTrailingReturn};

/// (`remove_trailing_return.rs:12-37`).
pub(crate) fn remove_trailing_return(
    _ctx: &DiagnosticsContext<'_>,
    d: &RemoveTrailingReturn,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unreachable-code", Severity::Hint),
        "unnecessary trailing `return`".to_string(),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix stub: remove the trailing `return`.
///
/// that the client resolves via `codeAction/resolve`.
fn fixes(d: &RemoveTrailingReturn) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    Some(vec![crate::unresolved_fix("remove_trailing_return", "Remove trailing `return`", range)])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag =
            AnyDiagnostic::RemoveTrailingReturn(RemoveTrailingReturn { node: test_node(0, 6) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("trailing `return`"));
    }
}
