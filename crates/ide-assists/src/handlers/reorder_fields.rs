//! `reorder_fields` assist — put a struct literal's fields in declaration order.
//!
//! ```sail
//! struct P = { x : int, y : int }
//! function f() = struct { y = 2, x = 1 }
//! ```
//! →
//! ```sail
//! function f() = struct { x = 1, y = 2 }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn reorder_fields(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let literal = root
        .descendants()
        .filter(|n| {
            n.kind() == SK::STRUCT_EXPR
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    let inits: Vec<(String, String)> = literal
        .children()
        .filter(|n| n.kind() == SK::FIELD_INIT)
        .filter_map(|f| {
            let name = f
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)?
                .text()
                .to_string();
            Some((name, f.text().to_string()))
        })
        .collect();
    if inits.len() < 2 {
        return None;
    }

    // The struct this literal builds, found the way `add_missing_fields`
    // finds it: Sail's literal does not name its type, so it is the one
    // struct that declares every field written.
    let item_tree = ctx.file.item_tree()?;
    let written: std::collections::HashSet<&str> = inits.iter().map(|(n, _)| n.as_str()).collect();
    let mut candidates = item_tree
        .top_level_items()
        .iter()
        .filter(|&&id| matches!(id.def_kind(item_tree), hir::DefKind::Struct))
        .map(|&id| id.fields(item_tree).iter().map(|f| f.name.to_string()).collect::<Vec<_>>())
        .filter(|fields| {
            !fields.is_empty() && written.iter().all(|w| fields.iter().any(|d| d == w))
        });
    let declared = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }

    let mut sorted = inits.clone();
    sorted.sort_by_key(|(n, _)| declared.iter().position(|d| d == n).unwrap_or(usize::MAX));
    if sorted == inits {
        return None; // already in order
    }

    let first = literal.children().find(|n| n.kind() == SK::FIELD_INIT)?;
    let last = literal.children().filter(|n| n.kind() == SK::FIELD_INIT).last()?;
    let range = base_db::text_range(
        usize::from(first.text_range().start()),
        usize::from(last.text_range().end()),
    );
    let replacement = sorted.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", ");
    acc.add(
        AssistId("reorder_fields", AssistKind::RefactorRewrite),
        "Reorder fields into declaration order",
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
            reorder_fields,
            "struct P = { x : int, y : int }\nval f : unit -> P\nfunction f() = struct { y$0 = 2, x = 1 }\n",
            expect![[r#"
                struct P = { x : int, y : int }
                val f : unit -> P
                function f() = struct { x = 1, y = 2 }
            "#]],
        );
    }

    /// Nothing to do when they are already in order.
    #[test]
    fn declines_when_ordered() {
        check_assist_not_applicable(
            reorder_fields,
            "struct P = { x : int, y : int }\nval f : unit -> P\nfunction f() = struct { x$0 = 1, y = 2 }\n",
        );
    }
}
