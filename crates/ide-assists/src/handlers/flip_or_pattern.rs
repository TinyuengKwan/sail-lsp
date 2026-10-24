//! `flip_or_pattern` assist — swap the two sides of an or-pattern.
//!
//! ```sail
//! match c { A | B => 1 }
//! ```
//! → (cursor on the `|`)
//! ```sail
//! match c { B | A => 1 }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn flip_or_pattern(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The `|` under the cursor, and the two patterns it separates. Sail's
    // or-pattern is a `BIN_PAT`, the same node shape a binary expression
    // has, so its two sides are its two child nodes.
    let pipe = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::PIPE
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let pat = pipe.parent().filter(|n| n.kind() == SK::BIN_PAT)?;

    let mut sides = pat.children();
    let (lhs, rhs) = (sides.next()?, sides.next()?);
    if sides.next().is_some() {
        return None;
    }

    let flipped = format!("{rhs} | {lhs}");
    let target = base_db::text_range(
        usize::from(pat.text_range().start()),
        usize::from(pat.text_range().end()),
    );
    acc.add(
        AssistId("flip_or_pattern", AssistKind::RefactorRewrite),
        "Flip or-pattern",
        target,
        |builder| builder.replace(target, flipped),
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
            flip_or_pattern,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A $0| B => 1 }\n",
            expect![[r#"
                enum C = A | B
                val f : C -> int
                function f(c) = match c { B | A => 1 }
            "#]],
        );
    }

    /// The `|` of an `enum` declaration is not an or-pattern.
    #[test]
    fn an_enum_bar_is_not_a_pattern() {
        check_assist_not_applicable(flip_or_pattern, "enum C = A $0| B\n");
    }
}
