//! `unwrap_tuple` assist — split a tuple binding into one binding per element.
//!
//! ```sail
//! let (a, b) = (1, 2)
//! ```
//! →
//! ```sail
//! let a = 1;
//! let b = 2
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn unwrap_tuple(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let let_expr = root
        .descendants()
        .filter(|n| {
            n.kind() == SK::LET_EXPR
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    let mut children = let_expr.children();
    let pat = children.next()?;
    let value = children.next()?;
    // Both sides have to be tuples of the same width, or there is nothing
    // to pair up.
    if pat.kind() != SK::TUPLE_PAT || value.kind() != SK::TUPLE_EXPR {
        return None;
    }
    let names: Vec<String> = pat.children().map(|n| n.text().to_string()).collect();
    let values: Vec<String> = value.children().map(|n| n.text().to_string()).collect();
    if names.len() < 2 || names.len() != values.len() {
        return None;
    }

    let bindings: Vec<String> =
        names.iter().zip(&values).map(|(n, v)| format!("let {n} = {v}")).collect();
    let replacement = bindings.join(";\n");

    let start = usize::from(let_expr.text_range().start());
    let end = usize::from(value.text_range().end());
    let range = base_db::text_range(start, end);
    acc.add(
        AssistId("unwrap_tuple", AssistKind::RefactorRewrite),
        "Unwrap tuple binding",
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
            unwrap_tuple,
            "function f() = { l$0et (a, b) = (1, 2); a + b }\n",
            expect![[r#"
                function f() = { let a = 1;
                let b = 2; a + b }
            "#]],
        );
    }

    /// A binding that is not a tuple has nothing to split.
    #[test]
    fn declines_on_a_plain_binding() {
        check_assist_not_applicable(unwrap_tuple, "function f() = { l$0et a = 1; a }\n");
    }
}
