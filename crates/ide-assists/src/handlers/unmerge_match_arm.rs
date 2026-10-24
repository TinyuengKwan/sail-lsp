//! `unmerge_match_arm` assist — split an or-pattern arm into one arm per pattern.
//!
//! ```sail
//! match c { A | B => 1, C => 2 }
//! ```
//! →
//! ```sail
//! match c { A => 1, B => 1, C => 2 }
//! ```
//!
//! The inverse of `merge_match_arms`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn unmerge_match_arm(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let arm = root
        .descendants()
        .filter(|n| {
            n.kind() == SK::MATCH_ARM
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    // A guard would have to be duplicated with the pattern, and whether
    // that is what was meant is not something to guess.
    if arm.children_with_tokens().filter_map(|el| el.into_token()).any(|t| t.kind() == SK::KW_IF) {
        return None;
    }
    let mut parts = arm.children();
    let pat = parts.next()?;
    let body = parts.next()?;
    if pat.kind() != SK::BIN_PAT {
        return None;
    }

    // `A | B | C` nests to the left, so the alternatives are the leaves.
    fn alternatives(node: &syntax::SyntaxNode, out: &mut Vec<String>) {
        if node.kind() == SK::BIN_PAT {
            for child in node.children() {
                alternatives(&child, out);
            }
        } else {
            out.push(node.text().to_string());
        }
    }
    let mut pats = Vec::new();
    alternatives(&pat, &mut pats);
    if pats.len() < 2 {
        return None;
    }

    let replacement = pats.iter().map(|p| format!("{p} => {body}")).collect::<Vec<_>>().join(", ");
    let range = base_db::text_range(
        usize::from(arm.text_range().start()),
        usize::from(arm.text_range().end()),
    );
    acc.add(
        AssistId("unmerge_match_arm", AssistKind::RefactorRewrite),
        "Split or-pattern into one arm each",
        range,
        |builder| builder.replace(range, replacement),
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
            unmerge_match_arm,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A $0| B => 1 }\n",
            expect![[r#"
                enum C = A | B
                val f : C -> int
                function f(c) = match c { A => 1, B => 1 }
            "#]],
        );
    }

    /// A guard would have to be duplicated along with the pattern.
    #[test]
    fn declines_on_a_guarded_arm() {
        check_assist_not_applicable(
            unmerge_match_arm,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A $0| B if true => 1 }\n",
        );
    }
}
