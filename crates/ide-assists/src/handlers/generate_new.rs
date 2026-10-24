//! `generate_new` assist — generate a constructor function for a struct.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_new(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let (struct_name, fields, insert_pos) =
        crate::utils::struct_at_offset(ctx.source_file()?, ctx.offset())?;

    let params: Vec<String> = fields.iter().map(|(n, t)| format!("{n} : {t}")).collect();
    let assignments: Vec<String> = fields.iter().map(|(n, _)| format!("{n} = {n}")).collect();

    let func = format!(
        "\nfunction mk_{}({}) -> {} = struct {{ {} }}",
        struct_name,
        params.join(", "),
        struct_name,
        assignments.join(", "),
    );

    acc.add_with_edits(
        AssistId("generate_new", AssistKind::Generate),
        "Generate constructor",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(insert_pos, insert_pos), new_text: func }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            generate_new,
            "struct Point = { x : int, y$0 : int }\n",
            expect![[r#"
            struct Point = { x : int, y : int }
            function mk_Point(x : int, y : int) -> Point = struct { x = x, y = y }
        "#]],
        );
    }

    /// The field list used to be split on `,`, so a type that has commas
    /// of its own read as several fields — all but the first without a
    /// `:`, and dropped.
    #[test]
    fn a_type_with_commas_in_it_is_one_field() {
        check_assist(
            generate_new,
            "struct S = { v$0 : vector(2, dec, bit) }\n",
            expect![[r#"
            struct S = { v : vector(2, dec, bit) }
            function mk_S(v : vector(2, dec, bit)) -> S = struct { v = v }
        "#]],
        );
    }
}
