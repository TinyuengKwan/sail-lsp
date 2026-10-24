//! `merge_nested_if` assist — merge nested `if` into a single `if` with `&` condition.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn merge_nested_if(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The `if` under the cursor.
    //
    // This used to search a ±100-byte window of raw text for the word
    // `if`, then for `then`, then count braces — so it matched those words
    // inside a string or a comment, and it took the *last* `if` in the
    // window rather than the one the cursor is on.
    let kw = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::KW_IF
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let outer = kw.parent().filter(|n| n.kind() == SK::IF_EXPR)?;

    let (outer_cond, outer_then, outer_else) = parts(&outer)?;
    // `if a then { if b then x } else y` is not `if (a & b) then x else y`:
    // the original runs `y` when `a` is false, the merged one also runs it
    // when `a` holds and `b` does not. The byte version replaced up to the
    // closing brace and left the `else` attached to the merged `if`.
    if outer_else.is_some() {
        return None;
    }

    let inner = unwrap_single(&outer_then)?;
    if inner.kind() != SK::IF_EXPR {
        return None;
    }
    let (inner_cond, inner_then, inner_else) = parts(&inner)?;
    if inner_else.is_some() {
        return None;
    }

    let merged = format!("if ({outer_cond} & {inner_cond}) then {inner_then}");
    acc.add(
        AssistId("merge_nested_if", AssistKind::RefactorRewrite),
        "Merge nested if",
        ctx.range,
        |builder| {
            let mut editor = builder.make_editor(&outer);
            editor.replace(outer.clone(), syntax::ast::make::expr_from_text(&merged));
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

/// An `IF_EXPR`'s condition, then-branch and else-branch, in that order —
/// which is the order they are its children in.
fn parts(
    if_expr: &syntax::SyntaxNode,
) -> Option<(syntax::SyntaxNode, syntax::SyntaxNode, Option<syntax::SyntaxNode>)> {
    let mut children = if_expr.children();
    let cond = children.next()?;
    let then = children.next()?;
    Some((cond, then, children.next()))
}

/// The one expression a branch holds, looking through a block that wraps
/// exactly one item.
fn unwrap_single(node: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut node = node.clone();
    loop {
        if !matches!(node.kind(), SK::BLOCK_EXPR | SK::BLOCK_ITEM) {
            return Some(node);
        }
        let mut children = node.children();
        let only = children.next()?;
        if children.next().is_some() {
            return None;
        }
        node = only;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            merge_nested_if,
            "function f(x, y, z) = $0if x then { if y then z }\n",
            expect![[r#"
            function f(x, y, z) = if (x & y) then z
        "#]],
        );
    }

    /// Merging past an `else` changes when the `else` runs.
    #[test]
    fn declines_when_the_outer_if_has_an_else() {
        check_assist_not_applicable(
            merge_nested_if,
            "function f(x, y, z, w) = $0if x then { if y then z } else w\n",
        );
    }

    /// The window search read the word `if` out of a string literal.
    #[test]
    fn a_string_holding_the_word_if_is_not_an_if() {
        check_assist_not_applicable(
            merge_nested_if,
            "function f(x, y, z) = { print(\"if y then\"); i$0f x then z }\n",
        );
    }
}
