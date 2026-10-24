//! `inline_type_alias` assist — replace a use of a type alias with what it aliases.
//!
//! ```sail
//! type Word = bits(32)
//! val f : Word -> Word
//! ```
//! → (cursor on a `Word`)
//! ```sail
//! val f : bits(32) -> Word
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn inline_type_alias(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // A type name under the cursor, in a type position.
    let token = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::IDENT
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let named = token.parent().filter(|n| n.kind() == SK::TYPE_NAMED)?;
    let name = token.text();

    // The alias it names, and what that alias stands for. The item tree
    // holds the signature; `type Word = bits(32)` signs as `bits(32)`.
    let item_tree = ctx.file.item_tree()?;
    let aliased = item_tree.top_level_items().iter().find_map(|&id| {
        (id.def_kind(item_tree) == hir::DefKind::TypeAlias && id.name(item_tree).as_str() == name)
            .then(|| id.signature(item_tree).to_string())
    })?;
    // The signature may be the whole declaration; take what follows the `=`.
    let rhs = aliased.split_once('=').map_or(aliased.trim(), |(_, r)| r.trim()).to_owned();
    if rhs.is_empty() || rhs == name {
        return None;
    }
    // Not on the alias's own declaration.
    if named.ancestors().any(|a| {
        a.kind() == SK::NAMED_DEF
            && a.children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::KW_TYPE)
    }) {
        return None;
    }

    let range = base_db::text_range(
        usize::from(named.text_range().start()),
        usize::from(named.text_range().end()),
    );
    acc.add(
        AssistId("inline_type_alias", AssistKind::RefactorInline),
        format!("Inline `{name}`"),
        range,
        |builder| builder.replace(range, rhs.clone()),
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist_not_applicable;

    /// Not on the alias's own declaration — that would delete it.
    #[test]
    fn declines_on_the_declaration() {
        check_assist_not_applicable(inline_type_alias, "type Wo$0rd = bits(32)\n");
    }

    /// A name that is not an alias has nothing to inline.
    #[test]
    fn declines_on_a_plain_type() {
        check_assist_not_applicable(inline_type_alias, "val f : in$0t -> int\n");
    }
}
