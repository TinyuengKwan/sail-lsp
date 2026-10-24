//! `add_missing_fields` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn add_missing_fields(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let offset = ctx.offset();

    // The tree the assist run already parsed. Parsing the text a second
    // time here built one unrelated to it.
    let cst_root = ctx.source_file()?.clone();

    // Find STRUCT_EXPR at cursor
    let struct_expr = cst_root.descendants().find(|node| {
        node.kind() == SK::STRUCT_EXPR && {
            let range = node.text_range();
            usize::from(range.start()) <= offset && offset <= usize::from(range.end())
        }
    })?;

    // The fields already written.
    let initialized_fields: std::collections::HashSet<String> = struct_expr
        .children()
        .filter(|c| c.kind() == SK::FIELD_INIT)
        .filter_map(|fi| {
            fi.descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| t.text().to_string())
        })
        .collect();

    // Which struct this literal builds.
    //
    // Sail's struct literal is `struct { f = v, … }` — it does not name
    // its type. This used to take the first `IDENT` inside the literal as
    // the struct's name, which is the first *field's* name, so it looked
    // for a definition called `x` and found none: the assist could not
    // fire at all. The type is the one struct that declares every field
    // written so far; if two do, there is nothing to go on and this
    // declines rather than guessing.
    let item_tree = ctx.file.item_tree()?;
    let mut candidates = item_tree
        .top_level_items()
        .iter()
        .filter(|&&id| matches!(id.def_kind(item_tree), hir::DefKind::Struct))
        .map(|&id| {
            let fields: Vec<String> =
                id.fields(item_tree).iter().map(|f| f.name.to_string()).collect();
            fields
        })
        .filter(|fields| {
            !fields.is_empty() && initialized_fields.iter().all(|f| fields.iter().any(|d| d == f))
        });
    let all_fields = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }

    // Find missing fields
    let missing: Vec<&String> =
        all_fields.iter().filter(|f| !initialized_fields.contains(f.as_str())).collect();

    if missing.is_empty() {
        return None;
    }

    // Each field's value is what has to be typed, so it is a tabstop for
    // a client that can expand snippets.
    let missing_text = match ctx.config.snippet_cap {
        Some(_) => missing
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{f} = ${{{}:()}}", i + 1))
            .collect::<Vec<_>>()
            .join(", "),
        None => missing.iter().map(|f| format!("{f} = ()")).collect::<Vec<_>>().join(", "),
    };

    // Find insertion point: before the closing `}`
    let close_brace = struct_expr
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::R_CURLY)
        .last()?;

    // Replacing the space before the `}` rather than inserting at it, so
    // the result reads `{ x = 1, y = () }` and not `{ x = 1 , y = ()}`.
    let insert_offset = usize::from(close_brace.text_range().start());
    let file_text = ctx.file.text();
    let before = file_text.get(..insert_offset)?.trim_end();
    let comma = if initialized_fields.is_empty() { "" } else { ", " };
    let text = format!("{comma}{missing_text} ");
    let range = base_db::text_range(before.len(), insert_offset);

    acc.add(
        AssistId("add_missing_fields", AssistKind::QuickFix),
        format!(
            "Add missing field(s): {}",
            missing.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ")
        ),
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.replace_snippet(range, text),
            None => builder.replace(range, text),
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
            add_missing_fields,
            "struct P = { x : int, y : int }\nval f : unit -> P\nfunction f() = struct { x$0 = 1 }\n",
            expect![[r#"
                struct P = { x : int, y : int }
                val f : unit -> P
                function f() = struct { x = 1, y = () }
            "#]],
        );
    }

    /// Each field's value is a tabstop, so you land on the first one.
    #[test]
    fn leaves_the_cursor_at_the_first_value() {
        check_assist_with_snippets(
            add_missing_fields,
            "struct P = { x : int, y : int }\nval f : unit -> P\nfunction f() = struct { x$0 = 1 }\n",
            expect![[r#"
                struct P = { x : int, y : int }
                val f : unit -> P
                function f() = struct { x = 1, y = ${1:()} }
            "#]],
        );
    }
}
