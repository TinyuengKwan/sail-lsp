//! Handler for `AnyDiagnostic::ConcatTypeMismatch`.
//!
//! Sail-specific: triggered when concatenation (`@`) operands are not
//! compatible vector types.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{ConcatTypeMismatch, DiagnosticsContext};

// Diagnostic: type-error
//
// This diagnostic is triggered when an operand of the concatenation operator
// `@` is not a vector: an integer, a boolean or a user type that is not a
// bitvector alias.
//
// ```sail
// function f(x : bits(4), n : int) -> bits(8) = x @ n
// ```
pub(crate) fn concat_type_mismatch(
    _ctx: &DiagnosticsContext<'_>,
    d: &ConcatTypeMismatch,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        d.message.clone(),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::ConcatTypeMismatch(ConcatTypeMismatch {
            message: "cannot concatenate int and bool".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("cannot concatenate int and bool"));
    }
}
