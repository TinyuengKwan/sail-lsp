//! Handler for `AnyDiagnostic::VectorSubrangeOrder`.
//!
//! Triggered when vector subrange indices violate the default bitvector order.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, VectorSubrangeOrder};

pub(crate) fn vector_subrange_order(
    _ctx: &DiagnosticsContext<'_>,
    d: &VectorSubrangeOrder,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        // Upstream's own words (`type_error.ml:301-315`): "First index 5
        // must be greater than or equal to second index 7 (when default
        // Order dec)".
        format!(
            "First index {} must be {} second index {} (when {})",
            d.first,
            if d.order_desc.contains("dec") {
                "greater than or equal to"
            } else {
                "less than or equal to"
            },
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
        assert!(d.unwrap().message.contains("First index"));
    }
}
