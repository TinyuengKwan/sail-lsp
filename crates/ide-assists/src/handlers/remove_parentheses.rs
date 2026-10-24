//! `remove_parentheses` assist — remove redundant parentheses.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn remove_parentheses(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The `(` or `)` under the cursor, and the node it belongs to. Counting
    // paren depth through the raw bytes instead — which is what stood here —
    // matches a parenthesis inside a string literal or a comment, and
    // "removes" a pair that was never one.
    let token = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        matches!(t.kind(), SK::L_PAREN | SK::R_PAREN)
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let node = token.parent()?;

    // Only a grouping paren. Sail's parser gives every parenthesised
    // expression a `TUPLE_EXPR`, and Sail has no one-element tuple, so a
    // single element inside is grouping and anything else is a tuple whose
    // parens are part of its meaning. This used to reject `TUPLE_EXPR`
    // outright, which is every paren there is — the assist could not fire.
    let mut children = node.children_with_tokens().filter(|el| !el.kind().is_trivia());
    let open = children.next()?.into_token().filter(|t| t.kind() == SK::L_PAREN)?;
    let inner = children.next()?;
    let close = children.next()?.into_token().filter(|t| t.kind() == SK::R_PAREN)?;
    if children.next().is_some() {
        return None;
    }

    // Through the tree: the parenthesised node is replaced by what it
    // wraps, and the text edit is whatever that turns out to be. Formatting
    // the inner node back into a string and computing the span of the pair
    // is the same answer only while nothing else moves.
    let _ = (&open, &close);
    acc.add(
        AssistId("remove_parentheses", AssistKind::RefactorRewrite),
        "Remove parentheses",
        ctx.range,
        |builder| {
            let mut editor = builder.make_editor(&node);
            // A detached copy: rowan will not splice a node that still
            // belongs to the tree being edited.
            editor.replace(node.clone(), syntax::ast::make::expr_from_text(&inner.to_string()));
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
            remove_parentheses,
            "val f : int -> int\nfunction f(x) = $0(x + 1)\n",
            expect![[r#"
            val f : int -> int
            function f(x) = x + 1
        "#]],
        );
    }
}
