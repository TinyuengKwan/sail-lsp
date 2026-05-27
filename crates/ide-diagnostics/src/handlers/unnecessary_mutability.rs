//! Handler for `AnyDiagnostic::UnnecessaryMutability`.
//!
//! Warning: mutable variable is never modified.
//! Fix: change `var` to `let` keyword.
//!
//! ## Future enhancement: mutable variable lifetime optimization
//!
//! A further refinement would detect `var` bindings assigned exactly once
//! after initialization where that assignment dominates all uses. In that
//! pattern, `var x` could be replaced with `let x = final_val`. This
//! requires counting assignments per variable and dataflow analysis.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, UnnecessaryMutability};

pub(crate) fn unnecessary_mutability(
    _ctx: &DiagnosticsContext<'_>,
    d: &UnnecessaryMutability,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("unmodified-mutable-variable", Severity::Warning),
        format!("variable `{}` is declared mutable but never modified", d.name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix: change `var` to `let`.
///
/// mutability keyword.
fn fixes(d: &UnnecessaryMutability) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    Some(vec![crate::unresolved_fix(
        "make_immutable",
        &format!("Change `{}` to immutable (`let`)", d.name),
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnnecessaryMutability(UnnecessaryMutability {
            name: "x".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("never modified"));
    }
}
