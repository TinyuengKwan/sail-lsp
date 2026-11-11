//! Handler for `AnyDiagnostic::NonContiguousSubrange`.
//!
//! Sail-specific: triggered when vector subrange patterns have gaps
//! between their bit ranges.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, NonContiguousSubrange};

// Diagnostic: type-error
//
// This diagnostic is triggered when a vector-concatenation pattern binds one
// name through several subranges that do not join up — `imm[15 .. 6] @ imm[4 ..
// 0]` leaves bit 5 undefined — or whose lowest subrange does not start at bit
// 0.
//
// ```sail
// default Order dec
// val unsigned : forall 'n. bits('n) -> int
// union ast = { I : bits(16) }
// function g(x : ast) -> int = match x {
//   I(imm[15 .. 6] @ imm[4 .. 0] @ rest[0 .. 0]) => unsigned(imm) + unsigned(rest)
// }
// ```
pub(crate) fn non_contiguous_subrange(
    _ctx: &DiagnosticsContext<'_>,
    d: &NonContiguousSubrange,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        // Upstream's own two messages (`type_check.ml:1791-1803`).
        match &d.problem {
            hir::SubrangeProblem::NotFromZero { lowest } => format!(
                "Cannot bind {} as pattern subranges do not start at bit 0 (lowest bit is {lowest}).",
                d.name
            ),
            hir::SubrangeProblem::Gap { missing } => format!(
                "Cannot bind {} as pattern subranges are non-contiguous. {}[{missing}] is not defined.",
                d.name, d.name
            ),
        },
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::NonContiguousSubrange(NonContiguousSubrange {
            name: "imm".to_string(),
            problem: hir::SubrangeProblem::NotFromZero { lowest: 5 },
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("do not start at bit 0"));
    }
}
