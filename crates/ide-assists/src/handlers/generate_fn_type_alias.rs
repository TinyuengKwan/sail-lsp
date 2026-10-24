//! `generate_fn_type_alias` assist — name a callable's type.
//!
//! ```sail
//! val f : (int, int) -> bool
//! ```
//! → (cursor on the name)
//! ```sail
//! type FType = (int, int) -> bool
//! val f : FType
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn generate_fn_type_alias(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let spec = root
        .descendants()
        .filter(|n| {
            n.kind() == SK::CALLABLE_SPEC
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    // The name is a `NAME` node, not a bare token on the spec.
    let name = spec.children().find(|n| n.kind() == SK::NAME)?.text().to_string();
    // The signature is everything after the `:`.
    let colon = spec
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::COLON)?;
    let sig_start = usize::from(colon.text_range().end());
    let sig_end = usize::from(spec.text_range().end());
    let text = ctx.file.text();
    let sig = text.get(sig_start..sig_end)?.trim().to_owned();
    if sig.is_empty() || !sig.contains("->") {
        return None;
    }

    let alias = format!("{}Type", name[..1].to_uppercase() + &name[1..]);
    let at = usize::from(spec.text_range().start());
    let sig_range = base_db::text_range(sig_start, sig_end);
    let target = base_db::text_range(at, at);
    let alias_line = format!("type {alias} = {sig}\n");
    let alias_use = format!(" {alias}");
    acc.add(
        AssistId("generate_fn_type_alias", AssistKind::Generate),
        format!("Generate `type {alias}`"),
        target,
        |builder| {
            // Back to front, so the first edit does not move the second.
            builder.replace(sig_range, alias_use.clone());
            builder.insert(at, alias_line.clone());
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
            generate_fn_type_alias,
            "val f$0 : (int, int) -> bool\n",
            expect![[r#"
                type FType = (int, int) -> bool
                val f : FType
            "#]],
        );
    }

    /// A spec whose type is not a function type has nothing to name.
    #[test]
    fn declines_without_an_arrow() {
        check_assist_not_applicable(generate_fn_type_alias, "val f$0 : int\n");
    }
}
