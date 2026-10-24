//! `generate_documentation_template` assist — generate a doc comment template for a function.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;

pub(crate) fn generate_documentation_template(
    acc: &mut Assists,
    ctx: &AssistContext<'_>,
) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();
    let root = ctx.source_file()?;

    // The function the cursor is in.
    //
    // The word `function` used to be looked for in a ±200-byte window —
    // the *last* one in it, comment and string literal included — then
    // the name by scanning to the first non-identifier character, the
    // parameters by counting parens, and the return type by searching the
    // remaining text for `->` and stopping at the first `=` or newline.
    // That last one takes the `=` of a default argument and the `->` of a
    // function type written as a parameter.
    let def = root
        .children()
        .filter(|n| {
            n.kind() == SK::CALLABLE_DEF
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;
    if !def
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| t.kind() == SK::KW_FUNCTION)
    {
        return None;
    }
    let func_start = usize::from(def.text_range().start());

    let func_name = def
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)?
        .text()
        .to_owned();

    // A parameter is `name : type`; the names are the `IDENT`s that a
    // `COLON` follows.
    let params: Vec<String> = def
        .children()
        .find(|n| n.kind() == SK::PARAM_LIST)
        .map(|list| {
            let elements: Vec<_> = list.children_with_tokens().collect();
            elements
                .iter()
                .enumerate()
                .filter_map(|(i, el)| {
                    let token = el.as_token()?;
                    if token.kind() != SK::IDENT {
                        return None;
                    }
                    let next = elements[i + 1..]
                        .iter()
                        .find(|el| !el.kind().is_trivia())
                        .map(|el| el.kind())?;
                    (next == SK::COLON).then(|| token.text().to_owned())
                })
                .collect()
        })
        .unwrap_or_default();

    // The return type is the node after `->`.
    let ret_ty = def
        .children_with_tokens()
        .skip_while(|el| el.kind() != SK::R_ARROW)
        .filter_map(|el| el.into_node())
        .next()
        .map(|n| n.text().to_string());

    let line_start = text[..func_start].rfind('\n').map_or(0, |p| p + 1);
    let indent = &text[line_start..func_start];

    let mut doc = String::new();
    doc.push_str(indent);
    doc.push_str("/** ");
    doc.push_str(&func_name);
    // The summary is the one part a template cannot write, so that is
    // where the cursor goes.
    if ctx.config.snippet_cap.is_some() {
        doc.push_str(" $0");
    }
    doc.push('\n');
    doc.push_str(indent);
    doc.push_str(" *\n");

    for name in &params {
        doc.push_str(indent);
        doc.push_str(" * @param ");
        doc.push_str(name);
        doc.push('\n');
    }

    if let Some(ref ty) = ret_ty {
        if !ty.is_empty() {
            doc.push_str(indent);
            doc.push_str(" * @returns ");
            doc.push_str(ty);
            doc.push('\n');
        }
    }

    doc.push_str(indent);
    doc.push_str(" */\n");

    acc.add(
        AssistId("generate_documentation_template", AssistKind::RefactorRewrite),
        "Generate documentation template",
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.insert_snippet(func_start, doc),
            None => builder.insert(func_start, doc),
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
            generate_documentation_template,
            "function f$0oo(x : int, y : bits(32)) -> bool = true\n",
            expect![[r#"
            /** foo
             *
             * @param x
             * @param y
             * @returns bool
             */
            function foo(x : int, y : bits(32)) -> bool = true
        "#]],
        );
    }

    /// The summary is the one line the template cannot write.
    #[test]
    fn leaves_the_cursor_in_the_summary() {
        crate::tests::check_assist_with_snippets(
            generate_documentation_template,
            "function f$0oo(x : int) -> bool = true\n",
            expect![[r#"
                /** foo $0
                 *
                 * @param x
                 * @returns bool
                 */
                function foo(x : int) -> bool = true
            "#]],
        );
    }

    /// The window search took the last `function` in ±200 bytes, wherever
    /// it was.
    #[test]
    fn a_comment_naming_a_function_is_not_one() {
        check_assist_not_applicable(
            generate_documentation_template,
            "// a comment about function bar$0\n",
        );
    }
}
