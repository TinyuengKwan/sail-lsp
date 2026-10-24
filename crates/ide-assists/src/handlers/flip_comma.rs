//! `flip_comma` assist — swap items around a comma.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn flip_comma(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The comma under the cursor, and the two elements it separates.
    //
    // Scanning the raw bytes outward — which is what stood here — cannot
    // tell which comma it has found. It walked back to the nearest `(`,
    // `{` or `[`, so in `f(g(a, b), c)` the cursor on the outer comma
    // took `g(`'s paren as the left edge and swapped `a, b)` with `c`.
    // Siblings in the tree are the two things a comma actually separates.
    let comma = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::COMMA
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;

    let mut prev = comma.prev_sibling_or_token();
    while prev.as_ref().is_some_and(|el| el.kind().is_trivia()) {
        prev = prev.and_then(|el| el.prev_sibling_or_token());
    }
    let mut next = comma.next_sibling_or_token();
    while next.as_ref().is_some_and(|el| el.kind().is_trivia()) {
        next = next.and_then(|el| el.next_sibling_or_token());
    }
    let (prev, next) = (prev?, next?);
    if matches!(prev.kind(), SK::COMMA | SK::L_PAREN) || next.kind() == SK::COMMA {
        return None;
    }

    // Both sides have to survive the round trip through `ast::make`.
    // `struct S = { a : int, b : bool }` has a comma too, and its siblings
    // are field declarations: `expr_from_text("a : int")` does not give one
    // back, so the flip produced source that does not parse.
    let (prev_copy, next_copy) = (detached(&prev), detached(&next));
    if prev_copy.to_string() != prev.to_string() || next_copy.to_string() != next.to_string() {
        return None;
    }

    let parent = comma.parent()?;
    // Not every comma separates two of a kind. In `forall 'n, 'n > 0.` it
    // separates the quantifier list from the constraint, and flipping gives
    // `forall 'n > 0, 'n.` — which does not parse. The quantifier list is a
    // tree now, so this is a question the syntax can answer.
    if matches!(parent.kind(), SK::TYPE_FORALL | SK::TYPE_EXISTENTIAL) {
        return None;
    }
    acc.add(
        AssistId("flip_comma", AssistKind::RefactorRewrite),
        "Flip comma",
        ctx.range,
        |builder| {
            let mut editor = builder.make_editor(&parent);
            // Each side is replaced by a *detached* copy of the other:
            // rowan will not splice an element that still belongs to the
            // tree being edited.
            editor.replace(prev.clone(), detached(&next));
            editor.replace(next.clone(), detached(&prev));
            let _ = (&prev_copy, &next_copy);
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

/// A copy of `el` that belongs to no tree, so it can be spliced into one.
fn detached(el: &syntax::SyntaxElement) -> syntax::SyntaxElement {
    syntax::SyntaxElement::Node(syntax::ast::make::expr_from_text(&el.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            flip_comma,
            "val f : (int, int) -> int\nfunction f(x$0, y) = x\n",
            expect![[r#"
            val f : (int, int) -> int
            function f(y, x) = x
        "#]],
        );
    }

    /// The byte scan walked back to the nearest opening bracket, which in
    /// a nested call is the *inner* one.
    #[test]
    fn a_nested_call_flips_the_arguments_it_is_on() {
        check_assist(
            flip_comma,
            "val f : (int, int) -> int\nval g : (int, int) -> int\nfunction h() = f(g(1, 2)$0, 3)\n",
            expect![[r#"
            val f : (int, int) -> int
            val g : (int, int) -> int
            function h() = f(3, g(1, 2))
        "#]],
        );
    }
}
