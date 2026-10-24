//! `add_return_type` assist — write a function's return type in.
//!
//! ```sail
//! function f(x) = x + 1
//! ```
//! → (cursor on the name, and inference knows the body's type)
//! ```sail
//! function f(x) -> int = x + 1
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn add_return_type(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let def = root
        .children()
        .filter(|n| {
            n.kind() == SK::CALLABLE_DEF
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;
    // Already annotated.
    if def.children_with_tokens().any(|el| el.kind() == SK::R_ARROW) {
        return None;
    }
    let params = def.children().find(|n| n.kind() == SK::PARAM_LIST)?;

    // What the body infers to. Written where a `->` would go, which is
    // after the parameter list.
    let body = def.children().filter(|n| n.kind() != SK::PARAM_LIST).last()?;
    let range = body.text_range();
    let ty = ctx
        .file
        .cached_expr_type_text(parser::Span::new(range.start().into(), range.end().into()))?;
    if ty == "unknown" || ty == "error" {
        return None;
    }

    let at = usize::from(params.text_range().end());
    let target = base_db::text_range(at, at);
    acc.add(
        AssistId("add_return_type", AssistKind::RefactorRewrite),
        format!("Add return type `{ty}`"),
        target,
        |builder| builder.insert(at, format!(" -> {ty}")),
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist_not_applicable;

    /// Declines where the type is already written.
    #[test]
    fn declines_when_annotated() {
        check_assist_not_applicable(add_return_type, "function f$0(x : int) -> int = x + 1\n");
    }
}
