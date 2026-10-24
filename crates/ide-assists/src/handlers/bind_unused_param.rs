//! `bind_unused_param` assist — mark a parameter as deliberately unused.
//!
//! ```sail
//! function f(x, y) = y
//! ```
//! → (cursor on `x`)
//! ```sail
//! function f(_x, y) = y
//! ```
//!
//! rust-analyzer writes `let _ = x;` into the body, because Rust's warning
//! is about the binding. Sail's `unused-variable` check reads the name, so
//! the underscore is what silences it here.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn bind_unused_param(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let token = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::IDENT
            && usize::from(t.text_range().start()) <= offset
            // The cursor may sit just past the name, which is where a
            // click at the end of a word leaves it.
            && offset <= usize::from(t.text_range().end())
    })?;
    if token.text().starts_with('_') {
        return None;
    }
    // Inside a parameter list, and not the callable's own name.
    token.parent_ancestors().find(|n| n.kind() == SK::PARAM_LIST)?;

    let name = token.text().to_owned();
    // Only where it is unused: a mention anywhere else in the definition
    // means renaming it would not compile.
    let def = token.parent_ancestors().find(|n| n.kind() == SK::CALLABLE_DEF)?;
    let mentions = def
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::IDENT && t.text() == name)
        .count();
    if mentions != 1 {
        return None;
    }

    let at = usize::from(token.text_range().start());
    let range = base_db::text_range(at, at);
    acc.add(
        AssistId("bind_unused_param", AssistKind::QuickFix),
        format!("Mark `{name}` unused"),
        range,
        |builder| builder.insert(at, "_"),
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
            bind_unused_param,
            "function f(x$0, y) = y\n",
            expect![[r#"
                function f(_x, y) = y
            "#]],
        );
    }

    /// A parameter the body uses is not unused.
    #[test]
    fn declines_on_a_used_parameter() {
        check_assist_not_applicable(bind_unused_param, "function f(x$0, y) = x\n");
    }
}
