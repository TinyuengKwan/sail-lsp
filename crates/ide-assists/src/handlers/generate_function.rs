//! `generate_function` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_function(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // Parse CST to find a CALL_EXPR at cursor
    let (cst_root, _) = syntax::parse_text(text);

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

    // Check if this function already exists in the file
    let already_defined = cst_root.children().any(|child| {
        if !matches!(child.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
            return false;
        }
        child
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|tok| tok.kind() == SK::IDENT && tok.text() == name)
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

    let stub = format!("\n\nval {name} : {param_types} -> _\nfunction {name}{params} = ()\n");

    // Insert at end of file
    let insert_offset = text.len();
    let edit =
        TextEdit { range: base_db::text_range(insert_offset, insert_offset), new_text: stub };

    acc.add_with_edits(
        AssistId("generate_function", AssistKind::Generate),
        format!("Generate function `{name}`"),
        ctx.range,
        vec![edit],
    );
    Some(())
}

//
// Trigger: cursor on a STRUCT_EXPR where not all fields are present.
// Action: insert `field: ()` for each missing field.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(generate_function, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
