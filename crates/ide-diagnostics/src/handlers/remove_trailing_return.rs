//! Handler for `AnyDiagnostic::RemoveTrailingReturn`.
//! Emitted when a function body ends with an explicit `return expr`
//! that is redundant because Sail uses implicit return for the last
//! expression.
//!
//! Fix: remove the `return` keyword and keep its expression.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;
use syntax::ast::{self, AstNode};

use crate::{DiagnosticsContext, RemoveTrailingReturn};

// Diagnostic: remove-trailing-return
//
// This diagnostic is triggered when a function body ends in `return e`: the
// last expression of a body is its value, so the `return` says nothing. A quick
// fix removes the keyword.
//
// ```sail
// function f() -> int = { let x : int = 3; return x }
// ```
/// (`remove_trailing_return.rs:12-37`).
pub(crate) fn remove_trailing_return(
    ctx: &DiagnosticsContext<'_>,
    d: &RemoveTrailingReturn,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("remove-trailing-return", Severity::Hint),
        "unnecessary trailing `return`".to_string(),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(ctx, d))
}

/// Delete the `return` keyword and the space after it; `return x + 1`
/// becomes `x + 1`, which Sail's implicit return already evaluates to.
/// A `return` with no operand has nothing to leave behind.
fn fixes(ctx: &DiagnosticsContext<'_>, d: &RemoveTrailingReturn) -> Option<Vec<Assist>> {
    let file = ctx.file.filter(|file| file.file_id(ctx.db()) == d.node.file_id)?;
    let root = ctx.sema.parse(file)?;
    let return_expr =
        d.node.value.try_to_node(&root)?.ancestors().find_map(ast::ReturnExpr::cast)?;
    let value = return_expr.value()?;
    let target = return_expr.syntax().text_range();
    let edit = TextEdit {
        range: ide_db::line_index::TextRange::new(
            target.start(),
            value.syntax().text_range().start(),
        ),
        new_text: String::new(),
    };
    Some(vec![crate::fix(
        "remove_trailing_return",
        "Remove trailing `return`",
        SourceChange::from_text_edit(edit),
        target,
    )])
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

#[cfg(test)]
mod fix_tests {
    use crate::tests::check_fix;

    #[test]
    fn removes_the_keyword_and_keeps_the_expression() {
        check_fix(
            "val f : int -> int\nfunction f(x) = {\n  let y = x + 1;\n  return y + 1\n}\n",
            "remove-trailing-return",
            "remove_trailing_return",
            "val f : int -> int\nfunction f(x) = {\n  let y = x + 1;\n  y + 1\n}\n",
        );
    }

    #[test]
    fn removes_a_bare_return_body() {
        check_fix(
            "val f : int -> int\nfunction f(x) = return x\n",
            "remove-trailing-return",
            "remove_trailing_return",
            "val f : int -> int\nfunction f(x) = x\n",
        );
    }
}
