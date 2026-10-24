//! `remove_else_branches` assist — drop an `if`'s `else`.
//!
//! ```sail
//! if c then a else b
//! ```
//! → (cursor on the `else`)
//! ```sail
//! if c then a
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn remove_else_branches(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let kw = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::KW_ELSE
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let if_expr = kw.parent().filter(|n| n.kind() == SK::IF_EXPR)?;

    // An `if` with no `else` has the type of its `then` branch only where
    // that type is `unit`, so this changes what the expression means and is
    // offered, not applied on its own.
    let mut parts = if_expr.children();
    let _cond = parts.next()?;
    let then_branch = parts.next()?;
    parts.next()?;

    // Everything from the end of the then-branch to the end of the `if`.
    let start = usize::from(then_branch.text_range().end());
    let end = usize::from(if_expr.text_range().end());
    let range = base_db::text_range(start, end);
    let target = base_db::text_range(
        usize::from(kw.text_range().start()),
        usize::from(kw.text_range().end()),
    );
    acc.add(
        AssistId("remove_else_branches", AssistKind::RefactorRewrite),
        "Remove else branch",
        target,
        |builder| builder.replace(range, String::new()),
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            remove_else_branches,
            "function f(c) = if c then () $0else print(\"x\")\n",
            expect![[r#"
                function f(c) = if c then ()
            "#]],
        );
    }

    /// There is nothing to remove when the `if` has no `else`.
    #[test]
    fn declines_without_an_else() {
        check_assist_not_applicable(remove_else_branches, "function f(c) = if c the$0n ()\n");
    }
}
