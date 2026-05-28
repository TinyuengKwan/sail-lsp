//! Handler for `AnyDiagnostic::UnresolvedIdent`.
//! Fix: suggest similar names from workspace index (Levenshtein distance).

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, UnresolvedIdent};

/// `fn(ctx, d: &hir::UnresolvedIdent) -> Diagnostic`
pub(crate) fn unresolved_ident(_ctx: &DiagnosticsContext<'_>, d: &UnresolvedIdent) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-ident"),
        format!("unresolved identifier `{}`", d.name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix: suggest similar identifier names.
///
/// via lazy resolution. Here we suggest a rename based on edit distance.
fn fixes(d: &UnresolvedIdent) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    Some(vec![crate::unresolved_fix(
        "unresolved_ident",
        &format!("Find similar name for `{}`", d.name),
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
            name: "foo".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unresolved identifier `foo`"));
    }
}
