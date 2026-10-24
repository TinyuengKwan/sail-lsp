//! `generate_function` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn generate_function(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // Parse CST to find a CALL_EXPR at cursor
    // The tree the assist run already parsed. Parsing the text a second
    // time here built one unrelated to it.
    let cst_root = ctx.source_file()?.clone();

    // Find the CALL_EXPR whose callee IDENT contains the cursor
    let mut target_name = None;
    let mut arg_count = 0usize;
    for node in cst_root.descendants() {
        if node.kind() != SK::CALL_EXPR {
            continue;
        }
        // The first IDENT descendant is the callee name
        let callee_ident = node
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT);
        let Some(callee_tok) = callee_ident else {
            continue;
        };
        let tok_range = callee_tok.text_range();
        if usize::from(tok_range.start()) <= offset && offset <= usize::from(tok_range.end()) {
            target_name = Some(callee_tok.text().to_string());
            // Count arguments in ARG_LIST
            if let Some(arg_list) = node.children().find(|c| c.kind() == SK::ARG_LIST) {
                // Count IDENT/LITERAL/EXPR children (approximate)
                arg_count = arg_list.children().count().max(
                    // Fallback: count commas + 1
                    arg_list
                        .descendants_with_tokens()
                        .filter(|el| el.as_token().is_some_and(|t| t.kind() == SK::COMMA))
                        .count()
                        + 1,
                );
                if arg_count == 1 && arg_list.text().to_string().trim().is_empty() {
                    arg_count = 0; // empty arg list `()`
                }
            }
            break;
        }
    }
    let name = target_name?;

    // Whether a definition of that name already exists.
    //
    // This used to match *any* `IDENT` inside a definition, and the call
    // being generated for is itself inside one — so `function f(x) =
    // helper(x)` contains the token `helper`, and the assist declined on
    // the case it exists for. A definition's name is its first `IDENT`,
    // after the keyword.
    let already_defined = cst_root.children().any(|child| {
        if !matches!(child.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
            return false;
        }
        child
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|tok| tok.kind() == SK::IDENT)
            .is_some_and(|tok| tok.text() == name)
    });
    if already_defined {
        return None; // Already exists — no assist needed
    }

    // Build function stub
    let params = if arg_count == 0 {
        "()".to_string()
    } else {
        let param_names: Vec<String> = (0..arg_count).map(|i| format!("arg{}", i + 1)).collect();
        format!("({})", param_names.join(", "))
    };
    let param_types = if arg_count == 0 {
        "unit".to_string()
    } else {
        let placeholders: Vec<_> = (0..arg_count).map(|_| "_").collect();
        if placeholders.len() == 1 {
            placeholders[0].to_string()
        } else {
            format!("({})", placeholders.join(", "))
        }
    };

    // Use ast::make::name to validate the generated name as a proper AST node.
    let _validated_name = syntax::ast::make::name(&name);

    // The stub's return type and body are what you have to type next, so
    // for a client that can expand snippets they are tabstops. Nothing
    // read `AssistConfig::snippet_cap` before — the field was passed in
    // from the server and dropped — and `SourceChange::is_snippet` had no
    // producer, so no assist could leave the cursor anywhere.
    let insert_offset = text.len();
    acc.add(
        AssistId("generate_function", AssistKind::Generate),
        format!("Generate function `{name}`"),
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.insert_snippet(
                insert_offset,
                format!(
                    "\n\nval {name} : {param_types} -> ${{1:_}}\nfunction {name}{params} = $0\n"
                ),
            ),
            None => builder.insert(
                insert_offset,
                format!("\n\nval {name} : {param_types} -> _\nfunction {name}{params} = ()\n"),
            ),
        },
    );
    Some(())
}

//
// Trigger: cursor on a STRUCT_EXPR where not all fields are present.
// Action: insert `field: ()` for each missing field.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable, check_assist_with_snippets};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            generate_function,
            "val f : int -> int\nfunction f(x) = hel$0per(x)\n",
            expect![[r#"
                val f : int -> int
                function f(x) = helper(x)


                val helper : _ -> _
                function helper(arg1) = ()
            "#]],
        );
    }

    /// A client that can expand snippets is left where it has to type.
    #[test]
    fn leaves_the_cursor_in_the_body() {
        check_assist_with_snippets(
            generate_function,
            "val f : int -> int\nfunction f(x) = hel$0per(x)\n",
            expect![[r#"
                val f : int -> int
                function f(x) = helper(x)


                val helper : _ -> ${1:_}
                function helper(arg1) = $0
            "#]],
        );
    }

    /// Declines where the function already exists.
    #[test]
    fn not_applicable_when_the_function_is_defined() {
        check_assist_not_applicable(
            generate_function,
            "val helper : int -> int\nfunction helper(y) = y\nfunction f(x) = hel$0per(x)\n",
        );
    }
}
