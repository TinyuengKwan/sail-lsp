//! `add_missing_fields` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn add_missing_fields(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let offset = ctx.offset();

    // The tree the assist run already parsed. Parsing the text a second
    // time here built one unrelated to it.
    let cst_root = ctx.source_file()?.clone();

    // Find STRUCT_EXPR at cursor
    let struct_expr = cst_root
        .descendants()
        .filter(|node| {
            matches!(node.kind(), SK::STRUCT_EXPR | SK::STRUCT_PAT) && {
                let range = node.text_range();
                usize::from(range.start()) <= offset && offset <= usize::from(range.end())
            }
        })
        .min_by_key(|node| node.text_range().len())?;

    let is_pattern = struct_expr.kind() == SK::STRUCT_PAT;

    // Use the same instantiated field types as the diagnostic quick fix.
    // Matching declarations by their written field names guesses incorrectly
    // when records share fields, and cannot handle cross-file/generic records.
    let (sema, _, file) = ctx.sema()?;
    let missing = sema.missing_record_fields(file, struct_expr.text_range())?;
    if missing.is_empty() {
        return None;
    }
    let names = missing.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>();

    // Each field's value is what has to be typed, so it is a tabstop for
    // a client that can expand snippets.
    let missing_text = missing
        .iter()
        .enumerate()
        .map(|(i, (field, ty))| {
            let value = if is_pattern {
                "_".to_string()
            } else {
                ide_db::assists::default_value_for_type(ty)
            };
            if ctx.config.snippet_cap.is_some() {
                format!("{field} = ${{{}:{value}}}", i + 1)
            } else {
                format!("{field} = {value}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    // Find insertion point: before the closing `}`
    let close_brace = struct_expr
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::R_CURLY)
        .last()?;

    // Replacing the space before the `}` rather than inserting at it, so
    // the result reads `{ x = 1, y = 0 }` and not `{ x = 1 , y = 0}`.
    let insert_offset = usize::from(close_brace.text_range().start());
    let file_text = ctx.file.text();
    let before = file_text.get(..insert_offset)?.trim_end();
    let mut previous = close_brace.prev_token()?;
    while previous.kind().is_trivia() {
        previous = previous.prev_token()?;
    }
    let comma = if previous.text() == "{" {
        ""
    } else if previous.text() == "," {
        " "
    } else {
        ", "
    };
    let text = format!("{comma}{missing_text} ");
    let range = base_db::text_range(before.len(), insert_offset);

    acc.add(
        AssistId("add_missing_fields", AssistKind::QuickFix),
        format!("Add missing field(s): {}", names.join(", ")),
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
                function f() = struct { x = 1, y = 0 }
            "#]],
        );
    }

    #[test]
    fn missing_pattern_fields_do_not_constrain_the_match() {
        check_assist(
            add_missing_fields,
            "struct P = { x : int, y : bool }\nval f : P -> int\nfunction f(p) = match p { struct { x$0 = n } => n }\n",
            expect![[r#"
                struct P = { x : int, y : bool }
                val f : P -> int
                function f(p) = match p { struct { x = n, y = _ } => n }
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
                function f() = struct { x = 1, y = ${1:0} }
            "#]],
        );
    }
}
