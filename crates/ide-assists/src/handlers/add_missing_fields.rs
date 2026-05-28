//! `add_missing_fields` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn add_missing_fields(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    let (cst_root, _) = syntax::parse_text(text);

    // Find STRUCT_EXPR at cursor
    let struct_expr = cst_root.descendants().find(|node| {
        node.kind() == SK::STRUCT_EXPR && {
            let range = node.text_range();
            usize::from(range.start()) <= offset && offset <= usize::from(range.end())
        }
    })?;

    // Extract struct name from the STRUCT_EXPR
    let struct_name = struct_expr
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)?
        .text()
        .to_string();

    // Collect already-initialized field names from FIELD_INIT children
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

    // Find the struct definition to get all field names
    let item_tree = ctx.file.item_tree()?;
    let mut all_fields: Vec<String> = Vec::new();

    for &id in item_tree.top_level_items() {
        if id.name(item_tree).as_str() == struct_name
            && matches!(id.item_kind(item_tree), hir_def::item_tree::ItemKind::Struct)
        {
            // Extract field names from the definition text
            let id_span = id.span(item_tree);
            let def_text = text.get(id_span.start..id_span.end)?;
            // Find text between { and } — each line with `:` is a field
            if let Some(brace_start) = def_text.find('{') {
                if let Some(brace_end) = def_text.rfind('}') {
                    let fields_text = &def_text[brace_start + 1..brace_end];
                    for line in fields_text.split(',') {
                        let line = line.trim();
                        if let Some(colon_pos) = line.find(':') {
                            let field_name = line[..colon_pos].trim();
                            if !field_name.is_empty() {
                                all_fields.push(field_name.to_string());
                            }
                        }
                    }
                }
            }
            break;
        }
    }

    // Find missing fields
    let missing: Vec<&String> =
        all_fields.iter().filter(|f| !initialized_fields.contains(f.as_str())).collect();

    if missing.is_empty() {
        return None;
    }

    // Build the insertion text
    let missing_text = missing.iter().map(|f| format!("{f} = ()")).collect::<Vec<_>>().join(", ");

    // Find insertion point: before the closing `}`
    let close_brace = struct_expr
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::R_CURLY)
        .last()?;

    let insert_offset = usize::from(close_brace.text_range().start());
    let comma = if initialized_fields.is_empty() { "" } else { ", " };
    let edit = TextEdit {
        range: base_db::text_range(insert_offset, insert_offset),
        new_text: format!("{comma}{missing_text}"),
    };

    acc.add_with_edits(
        AssistId("add_missing_fields", AssistKind::QuickFix),
        format!(
            "Add missing field(s): {}",
            missing.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ")
        ),
        ctx.range,
        vec![edit],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(add_missing_fields, "struct Foo { x : int }\n", 0);
        let _ = labels;
    }
}
