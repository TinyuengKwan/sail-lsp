//! `merge_match_arms` assist.
//! Merges consecutive match arms that have the same body expression
//! into a single arm with an OR pattern:
//!
//! ```sail
//! // Before:
//! match x {
//!     A => expr,
//!     B => expr,
//!     C => other,
//! }
//!
//! // After:
//! match x {
//!     A | B => expr,
//!     C => other,
//! }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

/// Merge consecutive match arms with identical bodies.
pub(crate) fn merge_match_arms(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let offset = ctx.offset();
    let root = ctx.source_file()?;

    // The innermost match the cursor is in.
    //
    // The arms used to be recovered by slicing between the first `{` and
    // the last `}` of the match's text and splitting on `,`, so a single
    // arm like `A(x, y) => e` read as two, and a guard's `=>`-less text
    // was dropped by the filter that followed.
    let match_node = root
        .descendants()
        .filter(|node| {
            node.kind() == SK::MATCH_EXPR && {
                let range = node.text_range();
                usize::from(range.start()) <= offset && offset <= usize::from(range.end())
            }
        })
        .last()?;

    let scrutinee = match_node.children().next()?;
    let arms: Vec<_> = match_node
        .children()
        .filter(|n| n.kind() == SK::MATCH_ARM)
        .map(|arm| {
            let has_guard = arm
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::KW_IF);
            let parts: Vec<_> = arm.children().collect();
            (arm, has_guard, parts)
        })
        .collect();
    if arms.len() < 2 {
        return None;
    }

    // Group consecutive arms by body. A guarded arm is its own group:
    // `A if p => e` and `B => e` run in different cases, so `A | B => e`
    // is not what either of them said.
    let mut groups: Vec<(Vec<String>, String, bool)> = Vec::new();
    for (_, has_guard, parts) in &arms {
        let pat = parts.first()?.to_string();
        let body = parts.last()?.to_string();
        let head = if *has_guard { format!("{pat} if {}", parts.get(1)?) } else { pat };
        match groups.last_mut() {
            Some(last) if last.1 == body && !last.2 && !has_guard => last.0.push(head),
            _ => groups.push((vec![head], body, *has_guard)),
        }
    }
    if groups.iter().all(|(pats, _, _)| pats.len() == 1) {
        return None;
    }

    // One line in, one line out.
    let multiline = match_node.text().to_string().contains('\n');
    let merged: Vec<(String, String)> =
        groups.into_iter().map(|(pats, body, _)| (pats.join(" | "), body)).collect();
    let arms_text = if multiline {
        merged.iter().map(|(p, b)| format!("\n    {p} => {b},")).collect::<String>() + "\n"
    } else {
        merged.iter().map(|(p, b)| format!("{p} => {b}")).collect::<Vec<_>>().join(", ")
    };
    let replacement = if multiline {
        format!("match {scrutinee} {{{arms_text}}}")
    } else {
        format!("match {scrutinee} {{ {arms_text} }}")
    };

    acc.add(
        AssistId("merge_match_arms", AssistKind::RefactorRewrite),
        "Merge match arms with same body",
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
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            merge_match_arms,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = $0match c { A => 1, B => 1 }\n",
            expect![[r#"
            enum C = A | B
            val f : C -> int
            function f(c) = match c { A | B => 1 }
        "#]],
        );
    }

    /// Splitting the arms on `,` read `A(x, y) => 1` as two arms, the
    /// second of which had no `=>` and was dropped.
    #[test]
    fn a_comma_inside_a_pattern_does_not_start_an_arm() {
        check_assist(
            merge_match_arms,
            "union C = A : (int, int) | B\nval f : C -> int\nfunction f(c) = $0match c { A(x, y) => 1, B => 1 }\n",
            expect![[r#"
            union C = A : (int, int) | B
            val f : C -> int
            function f(c) = match c { A(x, y) | B => 1 }
        "#]],
        );
    }

    /// `A if p => e` and `B => e` do not run in the same cases.
    #[test]
    fn a_guarded_arm_does_not_merge() {
        check_assist_not_applicable(
            merge_match_arms,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = $0match c { A if true => 1, B => 1 }\n",
        );
    }
}
