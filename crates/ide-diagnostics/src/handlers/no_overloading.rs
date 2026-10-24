//! Handler for `AnyDiagnostic::NoOverloading`.
//!
//! Diagnostic: no-overloading
//!
//! Sail-specific: triggered when no overload candidate matches a
//! function call. Conceptually.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;

use crate::{DiagnosticsContext, NoOverloading};

pub(crate) fn no_overloading(_ctx: &DiagnosticsContext<'_>, d: &NoOverloading) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("type-error"),
        // Two upstream errors, not one. `Err_no_function_type` is
        // "Function {name} not found", with the nearest name it does know
        // appended (`type_error.ml:316-322`); `Err_no_overloading` is the
        // one where candidates exist and none matched. Everything here was
        // the second sentence, including the four `fail/` cases that are
        // the first.
        if d.no_function {
            match &d.closest {
                Some(other) => format!("Function {} not found. Did you mean {other}?", d.name),
                None => format!("Function {} not found", d.name),
            }
        } else {
            format!("no matching overload for `{}`", d.name)
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
        let diag = AnyDiagnostic::NoOverloading(NoOverloading {
            name: "my_func".to_string(),
            node: test_node(0, 7),
            no_function: false,
            closest: None,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("my_func"));
    }
}
