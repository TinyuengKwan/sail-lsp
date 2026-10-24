//! `extract_struct_from_enum_variant` assist — give a union constructor's
//! payload a named type.
//!
//! ```sail
//! union U = { A : (int, int), B : bool }
//! ```
//! → (cursor on `A`)
//! ```sail
//! struct AData = { field0 : int, field1 : int }
//! union U = { A : AData, B : bool }
//! ```
//!
//! RA's version works on an enum variant; Sail's enums carry no payload —
//! `enum_bar` is a list of bare ids — so the constructor with a type is a
//! union's, and that is what this extracts.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn extract_struct_from_enum_variant(
    acc: &mut Assists,
    ctx: &AssistContext<'_>,
) -> Option<()> {
    let root = ctx.source_file()?;
    let offset = ctx.offset();

    let token = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::IDENT
            && usize::from(t.text_range().start()) <= offset
            && offset <= usize::from(t.text_range().end())
    })?;
    let name = token.text().to_owned();

    // The union definition this sits in, and the constructor's type.
    let def = token.parent_ancestors().find(|n| {
        n.kind() == SK::NAMED_DEF
            && n.children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::KW_UNION)
    })?;
    let field = token.parent_ancestors().find(|n| n.kind() == SK::FIELD_INIT)?;
    let ty = field.children().next()?;
    // Only a tuple is worth naming; a single type already has a name.
    if ty.kind() != SK::TYPE_TUPLE {
        return None;
    }
    let members: Vec<String> = ty.children().map(|n| n.text().to_string()).collect();
    if members.len() < 2 {
        return None;
    }

    let struct_name = format!("{name}Data");
    let fields = members
        .iter()
        .enumerate()
        .map(|(i, t)| format!("field{i} : {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let decl = format!("struct {struct_name} = {{ {fields} }}\n");

    let ty_range = base_db::text_range(
        usize::from(ty.text_range().start()),
        usize::from(ty.text_range().end()),
    );
    let at = usize::from(def.text_range().start());
    acc.add(
        AssistId("extract_struct_from_enum_variant", AssistKind::RefactorExtract),
        format!("Extract `struct {struct_name}`"),
        ty_range,
        |builder| {
            // Back to front, so the insertion does not move the replacement.
            builder.replace(ty_range, struct_name.clone());
            builder.insert(at, decl.clone());
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
            extract_struct_from_enum_variant,
            "union U = { A$0 : (int, int), B : bool }\n",
            expect![[r#"
                struct AData = { field0 : int, field1 : int }
                union U = { A : AData, B : bool }
            "#]],
        );
    }

    /// A constructor whose payload is one type already has a name for it.
    #[test]
    fn declines_on_a_single_type() {
        check_assist_not_applicable(
            extract_struct_from_enum_variant,
            "union U = { A$0 : int, B : bool }\n",
        );
    }
}
