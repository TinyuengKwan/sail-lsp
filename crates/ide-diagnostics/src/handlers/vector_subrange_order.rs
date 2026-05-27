//! Handler for `AnyDiagnostic::VectorSubrangeOrder`.
//!
//! Triggered when vector subrange indices violate the default bitvector order.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, VectorSubrangeOrder};

pub(crate) fn vector_subrange_order(
    _ctx: &DiagnosticsContext<'_>,
    d: &VectorSubrangeOrder,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        format!(
            "first index {} must be {} second index {} ({})",
            d.first,
            if d.order_desc.contains("dec") { ">=" } else { "<=" },
            d.second,
            d.order_desc,
        ),
        crate::node_file_range(&d.node),
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::VectorSubrangeOrder(VectorSubrangeOrder {
            first: "7".to_string(),
            second: "0".to_string(),
            order_desc: "dec".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("first index"));
    }
}
