//! `expand_rest_pattern` assist — write out what a struct pattern's `_` covers.
//!
//! ```sail
//! match p { struct { x = a, _ } => a }
//! ```
//! →
//! ```sail
//! match p { struct { x = a, y = y } => a }
//! ```
//!
//! Sail's `fpat` has a wildcard form — `Under` giving `FP_wild`
//! (`parser.mly:572`) — which is the `..` this assist expands in RA.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn expand_rest_pattern(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The `_` under the cursor, inside a struct pattern.
    let under = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::UNDERSCORE
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let pat = under.parent_ancestors().find(|n| n.kind() == SK::STRUCT_PAT)?;

    // The fields already named.
    let named: Vec<String> = pat
        .children()
        .filter_map(|f| {
            f.children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| t.text().to_string())
        })
        .collect();

    // Which struct this is: the one declaring every field named, as
    // `add_missing_fields` decides it — Sail's pattern need not say.
    let item_tree = ctx.file.item_tree()?;
    let mut candidates = item_tree
        .top_level_items()
        .iter()
        .filter(|&&id| matches!(id.def_kind(item_tree), hir::DefKind::Struct))
        .map(|&id| id.fields(item_tree).iter().map(|f| f.name.to_string()).collect::<Vec<_>>())
        .filter(|fields| !fields.is_empty() && named.iter().all(|n| fields.contains(n)));
    let declared = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }

    let missing: Vec<String> =
        declared.iter().filter(|d| !named.contains(d)).map(|d| format!("{d} = {d}")).collect();
    if missing.is_empty() {
        return None;
    }

    let range = base_db::text_range(
        usize::from(under.text_range().start()),
        usize::from(under.text_range().end()),
    );
    let replacement = missing.join(", ");
    acc.add(
        AssistId("expand_rest_pattern", AssistKind::RefactorRewrite),
        "Expand the rest of the fields",
        range,
        |builder| builder.replace(range, replacement),
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist_not_applicable;

    /// Nothing is covered by the `_` when every field is written.
    #[test]
    fn declines_when_all_fields_are_named() {
        check_assist_not_applicable(
            expand_rest_pattern,
            "struct P = { x : int }\nfunction f(p) = match p { struct { x = a, $0_ } => a }\n",
        );
    }
}
