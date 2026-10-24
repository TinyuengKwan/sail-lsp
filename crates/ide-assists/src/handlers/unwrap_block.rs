//! `unwrap_block` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;
use syntax::ast::AstNode as _;

pub(crate) fn unwrap_block(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = unwrap_block_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("unwrap_block", AssistKind::RefactorRewrite),
        "Unwrap block",
        ctx.range,
        te,
    );
    Some(())
}

pub fn unwrap_block_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    use hir::Expr;
    let offset = base_db::range_start(range);
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let text = file.text();

    // Find an if/while/foreach at cursor and extract body span
    for (id, hir) in entry.body.iter_exprs() {
        let expr_span = match entry.source_map.expr_syntax(id) {
            Some(s) => s,
            None => continue,
        };
        if offset < expr_span.start || offset > expr_span.end {
            continue;
        }
        // Cursor must be near the keyword (first ~8 chars)
        if offset > expr_span.start + 8 {
            continue;
        }

        let body_span = match hir {
            Expr::If { then_branch, .. } => entry.source_map.expr_syntax(*then_branch),
            Expr::While { body, .. } | Expr::Foreach { body, .. } => {
                entry.source_map.expr_syntax(*body)
            }
            _ => continue,
        }?;

        let body_text = text.get(body_span.start..body_span.end)?;
        let trimmed = body_text.trim();
        // Try CST-level trivial-expression extraction first.
        let inner = if trimmed.starts_with('{') && trimmed.ends_with('}') {
            // Parse the body text and try extract_trivial_expression for single-item blocks.
            let (root, _) = syntax::parse_text(trimmed);
            let block = root.descendants().find_map(syntax::ast::BlockExpr::cast);
            if let Some(trivial) = block.as_ref().and_then(crate::utils::extract_trivial_expression)
            {
                trivial.syntax().text().to_string()
            } else {
                // Fallback: manual brace stripping.
                trimmed[1..trimmed.len() - 1].trim().to_string()
            }
        } else {
            trimmed.to_string()
        };
        return Some(vec![IdeTextEdit {
            range: base_db::text_range(expr_span.start, expr_span.end),
            new_text: inner,
        }]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    /// Keeping only the body is the point of this assist, not a defect:
    /// rust-analyzer's says it "removes if...else, for, while and loop
    /// control statements to just keep the body", and its example turns
    /// `if true { println!() }` into `println!()`. An earlier version of
    /// this comment called it a bug.
    fn applies() {
        check_assist(
            unwrap_block,
            "val f : int -> int
function f(x) = $0if x > 0 then { x } else { 0 }
",
            expect![[r#"
    val f : int -> int
    function f(x) = x
"#]],
        );
    }
}
