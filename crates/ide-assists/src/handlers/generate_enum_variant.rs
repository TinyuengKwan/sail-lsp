//! `generate_enum_variant` assist — add a missing variant to an enum.
//!
//! ```sail
//! enum C = A | B
//! function f(c) = match c { A => 1, D => 2 }
//! ```
//! → (cursor on `D`)
//! ```sail
//! enum C = A | B | D
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn generate_enum_variant(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // A name used as a pattern in a match arm.
    let token = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::IDENT
            && usize::from(t.text_range().start()) <= offset
            && offset <= usize::from(t.text_range().end())
    })?;
    let name = token.text().to_owned();
    token.parent_ancestors().find(|n| n.kind() == SK::MATCH_ARM)?;

    // The enum the other arms of this match name. Sail's `match` does not
    // qualify its variants, so the enum is the one declaring them.
    let arm_names: Vec<String> = token
        .parent_ancestors()
        .find(|n| n.kind() == SK::MATCH_EXPR)?
        .children()
        .filter(|n| n.kind() == SK::MATCH_ARM)
        .filter_map(|a| a.children().next().map(|p| p.text().to_string()))
        .collect();

    let item_tree = ctx.file.item_tree()?;
    let target_enum = item_tree.top_level_items().iter().find_map(|&id| {
        if !matches!(id.def_kind(item_tree), hir::DefKind::Enum) {
            return None;
        }
        let variants: Vec<String> =
            id.fields(item_tree).iter().map(|f| f.name.to_string()).collect();
        // Declares some arm of this match, but not this one.
        let claims = arm_names.iter().any(|a| variants.contains(a));
        (claims && !variants.contains(&name)).then(|| id.span(item_tree))
    })?;

    let at = target_enum.end;
    let range = base_db::text_range(at, at);
    acc.add(
        AssistId("generate_enum_variant", AssistKind::Generate),
        format!("Add `{name}` to the enum"),
        range,
        |builder| builder.insert(at, format!(" | {name}")),
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
            generate_enum_variant,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A => 1, D$0 => 2 }\n",
            expect![[r#"
                enum C = A | B | D
                val f : C -> int
                function f(c) = match c { A => 1, D => 2 }
            "#]],
        );
    }

    /// A variant the enum already declares needs nothing.
    #[test]
    fn declines_on_a_known_variant() {
        check_assist_not_applicable(
            generate_enum_variant,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = match c { A => 1, B$0 => 2 }\n",
        );
    }
}
