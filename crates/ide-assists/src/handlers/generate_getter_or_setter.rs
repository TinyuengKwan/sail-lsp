//! `generate_getter_or_setter` assist — generate getter/setter functions for struct fields.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_getter_or_setter(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let (struct_name, fields, insert_pos) =
        crate::utils::struct_at_offset(ctx.source_file()?, ctx.offset())?;

    // Build getter and setter functions for each field
    let mut funcs = String::new();
    for (field_name, field_ty) in &fields {
        // Getter
        funcs.push_str(&format!(
            "\nfunction get_{}(s : {}) -> {} = s.{}",
            field_name, struct_name, field_ty, field_name,
        ));
        // Setter
        funcs.push_str(&format!(
            "\nfunction set_{}(s : {}, v : {}) -> {} = {{ s with {} = v }}",
            field_name, struct_name, field_ty, struct_name, field_name,
        ));
    }

    acc.add_with_edits(
        AssistId("generate_getter_or_setter", AssistKind::Generate),
        "Generate getters and setters",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(insert_pos, insert_pos), new_text: funcs }],
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
            generate_getter_or_setter,
            "struct Foo = { bar$0 : int }\n",
            expect![[r#"
            struct Foo = { bar : int }
            function get_bar(s : Foo) -> int = s.bar
            function set_bar(s : Foo, v : int) -> Foo = { s with bar = v }
        "#]],
        );
    }

    /// The field list used to be split on `,`, so a type that has commas
    /// of its own read as several fields.
    #[test]
    fn a_type_with_commas_in_it_is_one_field() {
        check_assist(
            generate_getter_or_setter,
            "struct S = { v$0 : vector(2, dec, bit) }\n",
            expect![[r#"
            struct S = { v : vector(2, dec, bit) }
            function get_v(s : S) -> vector(2, dec, bit) = s.v
            function set_v(s : S, v : vector(2, dec, bit)) -> S = { s with v = v }
        "#]],
        );
    }
}
