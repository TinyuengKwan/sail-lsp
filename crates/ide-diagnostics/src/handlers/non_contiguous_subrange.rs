//! Handler for `AnyDiagnostic::NonContiguousSubrange`.
//!
//! Diagnostic: non-contiguous-subrange
//!
//! Sail-specific: triggered when vector subrange patterns have gaps
//! between their bit ranges.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, NonContiguousSubrange};

pub(crate) fn non_contiguous_subrange(
    _ctx: &DiagnosticsContext<'_>,
    d: &NonContiguousSubrange,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        "pattern subranges are non-contiguous".to_string(),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag =
            AnyDiagnostic::NonContiguousSubrange(NonContiguousSubrange { node: test_node(0, 5) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("non-contiguous"));
    }
}
