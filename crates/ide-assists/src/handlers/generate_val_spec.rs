//! `generate_val_spec` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_val_spec(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // Parse CST and find the callable at cursor
    let (cst_root, _) = syntax::parse_text(text);

    // Find the CALLABLE_DEF node containing the cursor
    let mut target_node = None;
    for child in cst_root.children() {
        if child.kind() != SK::CALLABLE_DEF {
            continue;
        }
        let range = child.text_range();
        if usize::from(range.start()) <= offset && offset <= usize::from(range.end()) {
            target_node = Some(child);
            break;
        }
    }
    let node = target_node?;

    // Extract function name from the node
    let mut func_name = None;
    let mut is_function = false;
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if tok.kind() == SK::KW_FUNCTION {
            is_function = true;
        }
        if tok.kind() == SK::IDENT && func_name.is_none() {
            func_name = Some(tok.text().to_string());
            break;
        }
    }
    if !is_function {
        return None;
    }
    let name = func_name?;

    // Check if a val spec already exists for this name
    let val_exists = cst_root.children().any(|child| {
        if child.kind() != SK::CALLABLE_SPEC {
            return false;
        }
        child
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|tok| tok.kind() == SK::IDENT && tok.text() == name)
    });
    if val_exists {
        return None; // Already has a val spec
    }

    // Extract parameter names from PARAM_LIST
    let mut params = Vec::new();
    for child in node.children() {
        if child.kind() == SK::PARAM_LIST {
            for tok in child.descendants_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() == SK::IDENT {
                    params.push(tok.text().to_string());
                }
            }
            break;
        }
    }

    // Build the val spec text
    let param_types = if params.is_empty() {
        "unit".to_string()
    } else {
        let placeholders: Vec<_> = params.iter().map(|_| "_".to_string()).collect();
        if placeholders.len() == 1 {
            placeholders[0].clone()
        } else {
            format!("({})", placeholders.join(", "))
        }
    };
    let val_text = format!("val {} : {} -> _\n", name, param_types);

    // Insert before the function definition
    let insert_offset = usize::from(node.text_range().start());
    let edit =
        TextEdit { range: base_db::text_range(insert_offset, insert_offset), new_text: val_text };

    acc.add_with_edits(
        AssistId("generate_val_spec", AssistKind::Generate),
        format!("Generate `val {name} : ...`"),
        ctx.range,
        vec![edit],
    );
    Some(())
}

//
// Trigger: cursor on a match expression where the scrutinee is a known
// enum/union type and not all variants are covered.
// Action: insert `VariantName => todo("..."),` for each missing arm.
//
// For sail-riscv: the `extension` enum has 94+ scattered variants.
// This assist collects all variants from workspace ItemTrees.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(generate_val_spec, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
