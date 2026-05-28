//! `add_missing_match_arms` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn add_missing_match_arms(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use hir_def::item_tree::ItemKind;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // Parse CST and find the MATCH_EXPR at cursor
    let (cst_root, _) = syntax::parse_text(text);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);

    // Find the callable body containing this offset
    let entry = bodies.entry_at_offset(offset)?;
    let body = &entry.body;
    let source_map = &entry.source_map;

    // Find the match expression at cursor
    let expr_id = source_map.expr_at_offset(offset)?;
    let hir_def::hir::Expr::Match { scrutinee, arms } = body.expr(expr_id)? else {
        return None;
    };

    // Get the scrutinee name (must be a simple ident for enum resolution)
    let _scrutinee_name = match body.expr(*scrutinee)? {
        hir_def::hir::Expr::Ident(name) => name.clone(),
        _ => return None, // Can't resolve complex scrutinee
    };

    // Try to find the enum/union type from the workspace ItemTrees
    // Look for the scrutinee's type via val spec or direct enum name
    let item_tree = ctx.file.item_tree()?;

    // Strategy: Look for an enum/union whose name matches the scrutinee's type.
    // For sail-riscv, patterns like `match ext { ... }` where ext is of type `extension`.
    // We need to find the enum type. Heuristic: check if there's a val spec that
    // tells us the parameter type, or if the scrutinee name IS an enum type.

    // Collect all enum/union variants from the workspace for potential matches.
    // For a real implementation, we'd use type inference to determine the exact type.
    // For now, use a heuristic: look for an enum whose name is similar to the scrutinee.

    // Collect all enum variants from workspace
    let mut all_variants: Vec<String> = Vec::new();
    let mut enum_name: Option<String> = None;

    // Check if the file has any enum definition
    for &id in item_tree.top_level_items() {
        if id.item_kind(item_tree) == ItemKind::Enum || id.item_kind(item_tree) == ItemKind::Union
        {
            // Check inline variants
            let sig = id.signature(item_tree);
            if let Some(brace_start) = sig.find('{') {
                if let Some(brace_end) = sig.rfind('}') {
                    let inner = &sig[brace_start + 1..brace_end];
                    let variants: Vec<&str> = inner
                        .split(',')
                        .map(|s| s.trim().split(':').next().unwrap_or("").trim())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !variants.is_empty() {
                        enum_name = Some(id.name(item_tree).as_str().to_string());
                        all_variants = variants.iter().map(|s| s.to_string()).collect();
                    }
                }
            }
        }
        // Also check scattered enum clauses (member_name field)
        if id.is_clause(item_tree) && id.member_name(item_tree).is_some() {
            if let Some(member) = id.member_name(item_tree) {
                if !all_variants.contains(&member.to_string()) {
                    all_variants.push(member.to_string());
                    if enum_name.is_none() {
                        enum_name = Some(id.name(item_tree).as_str().to_string());
                    }
                }
            }
        }
    }

    if all_variants.is_empty() {
        return None;
    }

    // Collect existing arm patterns
    let mut covered: Vec<String> = Vec::new();
    for arm in arms {
        if let Some(pat) = body.pat(arm.pat) {
            match pat {
                hir_def::hir::Pat::Bind(name) => {
                    if name == "_" {
                        return None; // Wildcard covers everything
                    }
                    covered.push(name.clone());
                }
                hir_def::hir::Pat::App { ctor, .. } => {
                    covered.push(ctor.clone());
                }
                hir_def::hir::Pat::Wild => return None,
                _ => {}
            }
        }
    }

    // Find missing variants
    let missing: Vec<&String> = all_variants.iter().filter(|v| !covered.contains(v)).collect();

    if missing.is_empty() {
        return None;
    }

    // Build the text to insert
    let match_span = source_map.expr_syntax(expr_id)?;
    // Find the closing brace of the match expression
    let match_text = text.get(match_span.start..match_span.end)?;
    let close_brace_offset = match_text.rfind('}')?;
    let insert_offset = match_span.start + close_brace_offset;

    // Determine indentation from existing arms
    let indent = "    "; // default 4 spaces
    let mut new_arms = String::new();
    for variant in &missing {
        new_arms.push_str(&format!("{indent}{variant} => todo(\"implement {variant}\"),\n"));
    }

    let edit =
        TextEdit { range: base_db::text_range(insert_offset, insert_offset), new_text: new_arms };

    let label = format!("Fill {} missing match arms", missing.len());
    acc.add_with_edits(
        AssistId("add_missing_match_arms", AssistKind::QuickFix),
        label,
        ctx.range,
        vec![edit],
    );
    Some(())
}

//
// Trigger: cursor on a function call where the callee is not defined
// anywhere in the current file.
// Action: generate val spec + function stub at end of file.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels =
            check_assist(add_missing_match_arms, "function foo() -> unit = match x { }\n", 0);
        let _ = labels;
    }
}
