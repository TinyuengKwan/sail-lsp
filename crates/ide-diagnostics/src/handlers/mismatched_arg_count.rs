//! Handler for `AnyDiagnostic::MismatchedArgCount`.
//! Fix : unresolved fix to adjust argument count.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use ide_db::assists::Assist;

use crate::{DiagnosticsContext, MismatchedArgCount};

pub(crate) fn mismatched_arg_count(
    _ctx: &DiagnosticsContext<'_>,
    d: &MismatchedArgCount,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("mismatched-arg-count"),
        format!(
            "expected {} argument{}, found {}",
            d.expected,
            if d.expected == 1 { "" } else { "s" },
            d.found,
        ),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Unresolved fix: adjust argument count at the call site.
///
/// When too few args: suggest adding placeholder `()` arguments.
/// When too many args: suggest removing extra arguments.
fn fixes(d: &MismatchedArgCount) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    let label = if d.found < d.expected {
        format!("Add {} missing argument(s)", d.expected - d.found)
    } else {
        format!("Remove {} extra argument(s)", d.found - d.expected)
    };
    Some(vec![crate::unresolved_fix("fix_arg_count", &label, range)])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
            expected: 2,
            found: 3,
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected 2 arguments, found 3"));
    }

    #[test]
    fn singular_argument() {
        let diag = AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
            expected: 1,
            found: 0,
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected 1 argument,"));
    }
}
