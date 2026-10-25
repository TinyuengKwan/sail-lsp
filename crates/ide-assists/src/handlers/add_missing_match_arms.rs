//! `add_missing_match_arms` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn add_missing_match_arms(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.file.text();
    let offset = ctx.offset();
    let root = ctx.source_file()?;
    let node = root
        .descendants()
        .filter(|node| {
            node.kind() == syntax::SyntaxKind::MATCH_EXPR
                && usize::from(node.text_range().start()) <= offset
                && offset <= usize::from(node.text_range().end())
        })
        .min_by_key(|node| node.text_range().len())?;
    let (sema, _, file) = ctx.sema()?;
    let missing = sema.missing_match_patterns(file, node.text_range())?;
    if missing.is_empty() {
        return None;
    }
    let close = node
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|token| token.kind() == syntax::SyntaxKind::R_CURLY)?;
    let insert_offset = usize::from(close.text_range().start());
    let mut previous = close.prev_token()?;
    while previous.kind().is_trivia() {
        previous = previous.prev_token()?;
    }
    let needs_comma = !matches!(previous.text(), "{" | ",");

    // Each unfinished body is a snippet tabstop when the client supports it.
    let indent = "    ";
    let snippet = ctx.config.snippet_cap.is_some();
    let mut new_arms = String::new();
    for (i, variant) in missing.iter().enumerate() {
        let body =
            if snippet { format!("${{{}:undefined}}", i + 1) } else { "undefined".to_string() };
        new_arms.push_str(&format!("{indent}{variant} => {body},\n"));
    }

    // The arms go on their own lines, after a comma if the last existing
    // arm does not already end with one. Without this they were spliced
    // straight in front of the `}`: `match c { A => 1     B => todo(…),`,
    // which does not parse. Nothing noticed, because the assist could not
    // fire on a bar-separated enum at all.
    let before = text.get(..insert_offset)?.trim_end();
    let separator = if needs_comma { ",\n" } else { "\n" };
    let new_arms = format!("{separator}{new_arms}");
    // Replacing the whitespace before the `}` rather than inserting at it,
    // so the last arm does not end up with a space before its comma.
    let range = base_db::text_range(before.len(), insert_offset);

    let label = format!("Fill {} missing match arms", missing.len());
    acc.add(
        AssistId("add_missing_match_arms", AssistKind::QuickFix),
        label,
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.replace_snippet(range, new_arms),
            None => builder.replace(range, new_arms),
        },
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_with_snippets};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            add_missing_match_arms,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = mat$0ch c { A => 1 }\n",
            expect![[r#"
                enum C = A | B
                val f : C -> int
                function f(c) = match c { A => 1,
                    B => undefined,
                }
            "#]],
        );
    }

    #[test]
    fn uses_the_scrutinee_type_instead_of_the_last_enum() {
        check_assist(
            add_missing_match_arms,
            "enum C = A | B\nenum Other = X | Y\nval f : C -> int\nfunction f(c) = mat$0ch c { A => 1 }\n",
            expect![[r#"
                enum C = A | B
                enum Other = X | Y
                val f : C -> int
                function f(c) = match c { A => 1,
                    B => undefined,
                }
            "#]],
        );
    }

    #[test]
    fn retains_uncovered_constructor_payloads() {
        check_assist(
            add_missing_match_arms,
            "union U = { Some : bool, None : unit }\nval f : U -> int\nfunction f(c) = mat$0ch c { Some(true) => 1, None => 0 }\n",
            expect![[r#"
                union U = { Some : bool, None : unit }
                val f : U -> int
                function f(c) = match c { Some(true) => 1, None => 0,
                    Some(false) => undefined,
                }
            "#]],
        );
    }

    #[test]
    fn guarded_arm_does_not_cover_its_pattern_unconditionally() {
        check_assist(
            add_missing_match_arms,
            "val f : (bool, bool) -> int\nfunction f(c, ready) = mat$0ch c { true if ready => 1, false => 0 }\n",
            expect![[r#"
                val f : (bool, bool) -> int
                function f(c, ready) = match c { true if ready => 1, false => 0,
                    true => undefined,
                }
            "#]],
        );
    }

    #[test]
    fn nested_payload_witness_preserves_the_uncovered_value() {
        check_assist(
            add_missing_match_arms,
            "union Inner = { Inside : bool }\nunion Outer = { Outside : Inner }\nval f : Outer -> int\nfunction f(c) = mat$0ch c { Outside(Inside(true)) => 1 }\n",
            expect![[r#"
                union Inner = { Inside : bool }
                union Outer = { Outside : Inner }
                val f : Outer -> int
                function f(c) = match c { Outside(Inside(true)) => 1,
                    Outside(Inside(false)) => undefined,
                }
            "#]],
        );
    }

    /// A client that can expand snippets lands in the first arm it has to
    /// write, and tabs to the next.
    #[test]
    fn leaves_the_cursor_in_the_first_new_arm() {
        check_assist_with_snippets(
            add_missing_match_arms,
            "enum C = A | B | D\nval f : C -> int\nfunction f(c) = mat$0ch c { A => 1 }\n",
            expect![[r#"
                enum C = A | B | D
                val f : C -> int
                function f(c) = match c { A => 1,
                    B => ${1:undefined},
                    D => ${2:undefined},
                }
            "#]],
        );
    }
}
