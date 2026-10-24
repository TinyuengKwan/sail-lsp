//! `move_guard` assist — move match guard to/from arm body.
//! Two assists:
//! 1. `move_guard_to_arm_body`: Convert `pat if guard => body` to
//!    `pat => if guard then body else ()`
//! 2. `move_arm_cond_to_match_guard`: Convert `pat => if cond then body`
//!    to `pat if cond => body`
//!
//! Both used to walk the token stream: for the first `if` within ±100
//! bytes of the cursor, then forward for `=>`, then backwards past
//! whatever token happened to precede it to decide whether that `if` was a
//! guard or a body. A `MATCH_ARM` says which it is — a guard is a direct
//! child of the arm, a body `if` is the arm's body — so the question does
//! not arise.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;
use syntax::SyntaxNode;

/// Entry point: try both directions.
pub(crate) fn move_guard(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    move_guard_to_arm_body(acc, ctx).or_else(|| move_arm_cond_to_match_guard(acc, ctx))
}

/// The `MATCH_ARM` the cursor is in, and its pattern, guard and body.
struct Arm {
    node: SyntaxNode,
    pat: SyntaxNode,
    guard: Option<SyntaxNode>,
    body: SyntaxNode,
}

fn arm_at(ctx: &AssistContext<'_>) -> Option<Arm> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();
    let token = root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| {
            usize::from(t.text_range().start()) <= offset
                && offset < usize::from(t.text_range().end())
        })
        .or_else(|| {
            root.descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| usize::from(t.text_range().start()) >= offset)
        })?;
    let node = token.parent_ancestors().find(|n| n.kind() == SK::MATCH_ARM)?;

    // `pat [if guard] => body`: the guard is there iff the arm has a `KW_IF`
    // of its own.
    let has_guard =
        node.children_with_tokens().filter_map(|el| el.into_token()).any(|t| t.kind() == SK::KW_IF);
    let mut children = node.children();
    let pat = children.next()?;
    let (guard, body) = if has_guard {
        let guard = children.next()?;
        (Some(guard), children.next()?)
    } else {
        (None, children.next()?)
    };
    Some(Arm { node, pat, guard, body })
}

/// Move a match arm guard into the arm body as an `if` expression.
///
/// `pat if guard => body` becomes `pat => if guard then body else ()`.
/// rust-analyzer's version produces `if guard { expr }` and has the same
/// limitation — the arm only type-checks where its value is discarded.
/// The `else ()` makes that explicit rather than better.
fn move_guard_to_arm_body(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let arm = arm_at(ctx)?;
    let guard = arm.guard.as_ref()?;

    let new_arm = syntax::ast::make::match_arm(
        &arm.pat.to_string(),
        None,
        &format!("if {guard} then {} else ()", arm.body),
    );
    let target = base_db::text_range(
        usize::from(guard.text_range().start()),
        usize::from(guard.text_range().end()),
    );
    acc.add(
        AssistId("move_guard_to_arm_body", AssistKind::RefactorRewrite),
        "Move guard to arm body",
        target,
        |builder| {
            let mut editor = builder.make_editor(&arm.node);
            editor.replace(arm.node.clone(), new_arm);
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

/// Move an `if` condition from arm body to a match guard.
///
/// `pat => if cond then body` becomes `pat if cond => body`. An `else` is
/// what stops it: `pat => if c then a else b` matches the arm whatever `c`
/// says, and `pat if c => a` leaves the pattern to the arms below.
fn move_arm_cond_to_match_guard(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let arm = arm_at(ctx)?;
    if arm.guard.is_some() || arm.body.kind() != SK::IF_EXPR {
        return None;
    }
    let mut parts = arm.body.children();
    let cond = parts.next()?;
    let then_body = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let new_arm = syntax::ast::make::match_arm(
        &arm.pat.to_string(),
        Some(&cond.to_string()),
        &then_body.to_string(),
    );
    let target = base_db::text_range(
        usize::from(arm.body.text_range().start()),
        usize::from(arm.body.text_range().end()),
    );
    acc.add(
        AssistId("move_arm_cond_to_match_guard", AssistKind::RefactorRewrite),
        "Move condition to match guard",
        target,
        |builder| {
            let mut editor = builder.make_editor(&arm.node);
            editor.replace(arm.node.clone(), new_arm);
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    #[allow(clippy::needless_raw_string_hashes)]
    fn applies() {
        check_assist(
            move_guard,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A i$0f true => 1, B => 2 }\n",
            expect![[r#"
            enum C = A | B
            val f : C -> int
            function f(c) = match c { A => if true then 1 else (), B => 2 }
        "#]],
        );
    }

    /// The other direction, on an arm with no guard.
    #[test]
    fn moves_an_arm_condition_up_into_the_guard() {
        check_assist(
            move_guard,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A => i$0f true then 1, B => 2 }\n",
            expect![[r#"
            enum C = A | B
            val f : C -> int
            function f(c) = match c { A if true => 1, B => 2 }
        "#]],
        );
    }

    /// `A => if c then a else b` still matches `A` when `c` is false;
    /// `A if c => a` does not, and falls through to the arms below.
    #[test]
    fn declines_to_move_a_condition_that_has_an_else() {
        check_assist_not_applicable(
            move_guard,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A => i$0f true then 1 else 3, B => 2 }\n",
        );
    }
}
