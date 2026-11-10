//! Handler for `AnyDiagnostic::RemoveUnnecessaryElse`.
//! Emitted when an `if` expression's `then` branch diverges
//! (return/throw/exit), making the `else` branch unnecessary.
//! The code after the `if` can be written at the same level.
//!
//! Fix: remove the `else` and continue with its body as the next statement.

use crate::Diagnostic;
use hir::diagnostics::{DiagnosticCode, Severity};
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;
use syntax::ast::{self, AstNode};

use crate::{DiagnosticsContext, RemoveUnnecessaryElse};

/// (`remove_unnecessary_else.rs:21-41`).
pub(crate) fn remove_unnecessary_else(
    ctx: &DiagnosticsContext<'_>,
    d: &RemoveUnnecessaryElse,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailLint("remove-unnecessary-else", Severity::Warning),
        "unnecessary `else` because `then` branch diverges".to_string(),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(ctx, d))
}

/// `if c then return x else E` becomes `if c then return x; E`: the `then`
/// branch never falls through, so what the `else` guarded can follow the
/// `if` as the next statement. An `else` block is unwrapped and its lines
/// re-indented to the `if`; when the `if` is not already a block statement
/// the result is wrapped in braces, since the `;` needs a block to live in.
fn fixes(ctx: &DiagnosticsContext<'_>, d: &RemoveUnnecessaryElse) -> Option<Vec<Assist>> {
    let file = ctx.file.filter(|file| file.file_id(ctx.db()) == d.node.file_id)?;
    let root = ctx.sema.parse(file)?;
    let if_expr = d.node.value.try_to_node(&root)?.ancestors().find_map(ast::IfExpr::cast)?;
    let then_branch = if_expr.then_branch()?;
    let else_branch = if_expr.else_branch()?;
    let target = if_expr.syntax().text_range();
    let text = root.text().to_string();

    let indent = {
        let line_start = text[..usize::from(target.start())].rfind('\n').map_or(0, |i| i + 1);
        let line = &text[line_start..usize::from(target.start())];
        if line.trim().is_empty() {
            line
        } else {
            ""
        }
    };
    let continuation = match &else_branch {
        ast::Expr::BlockExpr(block) => reindent_block(block, indent),
        other => other.syntax().text().to_string(),
    };
    let continuation = continuation.trim_end_matches(';').trim_end().to_owned();
    if continuation.is_empty() {
        return None;
    }
    let in_block = if_expr
        .syntax()
        .parent()
        .is_some_and(|parent| parent.kind() == syntax::SyntaxKind::BLOCK_ITEM);
    let separator =
        if continuation.contains('\n') { format!("\n{indent}") } else { " ".to_owned() };
    let mut edits = vec![TextEdit {
        range: ide_db::line_index::TextRange::new(
            then_branch.syntax().text_range().end(),
            target.end(),
        ),
        new_text: format!(";{separator}{continuation}"),
    }];
    if !in_block {
        edits[0].new_text.push_str(" }");
        edits.push(TextEdit {
            range: ide_db::line_index::TextRange::empty(target.start()),
            new_text: "{ ".to_owned(),
        });
    }
    let mut source_change = SourceChange::new();
    source_change.insert_source_edit(d.node.file_id, edits);
    Some(vec![crate::fix(
        "remove_unnecessary_else",
        "Remove unnecessary `else`",
        source_change,
        target,
    )])
}

/// The statements inside `{ ... }`, with the block's own indentation
/// replaced by `indent`.
fn reindent_block(block: &ast::BlockExpr, indent: &str) -> String {
    let text = block.syntax().text().to_string();
    let inner =
        text.strip_prefix('{').and_then(|text| text.strip_suffix('}')).unwrap_or(&text).trim();
    let block_indent = inner
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    inner
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 || line.trim().is_empty() {
                line.trim_end().to_owned()
            } else {
                let stripped = &line[block_indent.min(line.len() - line.trim_start().len())..];
                format!("{indent}{stripped}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag =
            AnyDiagnostic::RemoveUnnecessaryElse(RemoveUnnecessaryElse { node: test_node(0, 4) });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("unnecessary `else`"));
    }
}

#[cfg(test)]
mod fix_tests {
    use crate::tests::check_fix;

    #[test]
    fn unwraps_the_else_block_into_the_enclosing_block() {
        check_fix(
            "val f : int -> int\nfunction f(x) = {\n  if x > 0 then return 1 else {\n    let y = 2;\n    y\n  }\n}\n",
            "remove-unnecessary-else",
            "remove_unnecessary_else",
            "val f : int -> int\nfunction f(x) = {\n  if x > 0 then return 1;\n  let y = 2;\n  y\n}\n",
        );
    }

    #[test]
    fn keeps_a_bare_else_expression_as_the_next_statement() {
        check_fix(
            "val f : int -> int\nfunction f(x) = { if x > 0 then return 1 else 2 }\n",
            "remove-unnecessary-else",
            "remove_unnecessary_else",
            "val f : int -> int\nfunction f(x) = { if x > 0 then return 1; 2 }\n",
        );
    }

    #[test]
    fn wraps_in_a_block_when_the_if_is_the_whole_body() {
        check_fix(
            "val f : int -> int\nfunction f(x) = if x > 0 then return 1 else 2\n",
            "remove-unnecessary-else",
            "remove_unnecessary_else",
            "val f : int -> int\nfunction f(x) = { if x > 0 then return 1; 2 }\n",
        );
    }
}
