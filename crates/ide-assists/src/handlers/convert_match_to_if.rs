//! `convert_match_to_if` assist.
//! Converts a match expression with a single non-wildcard arm + wildcard
//! into an if-then-else:
//!
//! ```sail
//! // Before:
//! match x {
//!     true => expr1,
//!     _ => expr2,
//! }
//!
//! // After:
//! if x then expr1 else expr2
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

/// Convert a two-arm match (pattern + wildcard) to if-then-else.
pub(crate) fn convert_match_to_if(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // The file's own tree. Parsing the text a second time here built a
    // tree with no relation to the one the rest of the assist layer holds.
    let cst_root = ctx.source_file()?.clone();

    // Find MATCH_EXPR at cursor.
    let match_node = cst_root.descendants().find(|node| {
        node.kind() == SK::MATCH_EXPR && {
            let range = node.text_range();
            usize::from(range.start()) <= offset && offset <= usize::from(range.end())
        }
    })?;

    // Parse the match via HIR to get structured arms.
    let bodies = hir::CallableBodies::from_cst(&cst_root);
    let entry = bodies.entry_at_offset(offset)?;
    let body = &entry.body;
    let source_map = &entry.source_map;
    let expr_id = source_map.expr_at_offset(offset)?;

    let hir::Expr::Match { scrutinee, arms } = body.expr(expr_id)? else {
        return None;
    };

    // Must have exactly 2 arms: one pattern + one wildcard.
    if arms.len() != 2 {
        return None;
    }

    let (pattern_arm, wildcard_arm) = {
        let arm0_pat = body.pat(arms[0].pat)?;
        let arm1_pat = body.pat(arms[1].pat)?;
        let arm0_is_wild = matches!(arm0_pat, hir::Pat::Wild)
            || matches!(arm0_pat, hir::Pat::Bind(name) if name == "_");
        let arm1_is_wild = matches!(arm1_pat, hir::Pat::Wild)
            || matches!(arm1_pat, hir::Pat::Bind(name) if name == "_");

        if arm0_is_wild && !arm1_is_wild {
            (&arms[1], &arms[0])
        } else if arm1_is_wild && !arm0_is_wild {
            (&arms[0], &arms[1])
        } else {
            return None; // Neither or both are wildcards
        }
    };

    // Extract text for scrutinee, pattern arm body, and wildcard arm body.
    let scrutinee_span = source_map.expr_syntax(*scrutinee)?;
    let scrutinee_text = text.get(scrutinee_span.start..scrutinee_span.end)?;

    let pat_span = source_map.pat_syntax(pattern_arm.pat)?;
    let pat_text = text.get(pat_span.start..pat_span.end)?;

    let then_span = source_map.expr_syntax(pattern_arm.body)?;
    let then_text = text.get(then_span.start..then_span.end)?;

    let else_span = source_map.expr_syntax(wildcard_arm.body)?;
    let else_text = text.get(else_span.start..else_span.end)?;

    // Build the condition. For simple patterns like `true`, we use
    // `scrutinee == pattern`; for constructor patterns, keep as match.
    let condition = if pat_text == "true" {
        scrutinee_text.to_string()
    } else if pat_text == "false" {
        format!("not({})", scrutinee_text)
    } else {
        // For constructor patterns (e.g., Some(x)), we can't easily
        // convert — only handle boolean-like patterns.
        format!("{} == {}", scrutinee_text, pat_text)
    };

    // Build replacement: `if <cond> then <then> else <else>`
    let replacement = format!("if {condition} then {then_text} else {else_text}");

    // Replace the whole match expression, through the tree.
    acc.add(
        AssistId("convert_match_to_if", AssistKind::RefactorRewrite),
        "Convert match to if-then-else",
        ctx.range,
        |builder| {
            let mut editor = builder.make_editor(&match_node);
            editor.replace(match_node.clone(), syntax::ast::make::expr_from_text(&replacement));
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            convert_match_to_if,
            "val f : bool -> int\nfunction f(b) = mat$0ch b { true => 1, _ => 2 }\n",
            expect![[r#"
            val f : bool -> int
            function f(b) = if b then 1 else 2
        "#]],
        );
    }

    #[test]
    fn a_false_arm_negates_the_scrutinee() {
        check_assist(
            convert_match_to_if,
            "val f : bool -> int\nfunction f(b) = mat$0ch b { false => 1, _ => 2 }\n",
            expect![[r#"
            val f : bool -> int
            function f(b) = if not(b) then 1 else 2
        "#]],
        );
    }
}
