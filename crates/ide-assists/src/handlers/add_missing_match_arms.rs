//! `add_missing_match_arms` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn add_missing_match_arms(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use hir::DefKind;

    let text = ctx.file.text();
    let offset = ctx.offset();

    // Parse CST and find the MATCH_EXPR at cursor
    // The tree the assist run already parsed. Parsing the text a second
    // time here built one unrelated to it.
    let cst_root = ctx.source_file()?.clone();
    let bodies = hir::CallableBodies::from_cst(&cst_root);

    // Find the callable body containing this offset
    let entry = bodies.entry_at_offset(offset)?;
    let body = &entry.body;
    let source_map = &entry.source_map;

    // Find the match expression at cursor
    let expr_id = source_map.expr_at_offset(offset)?;
    let hir::Expr::Match { scrutinee, arms } = body.expr(expr_id)? else {
        return None;
    };

    // Get the scrutinee name (must be a simple ident for enum resolution)
    let _scrutinee_name = match body.expr(*scrutinee)? {
        hir::Expr::Ident(name) => name.clone(),
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

    // The variants an enum or union declares.
    //
    // These used to be recovered by finding `{` and `}` in the item's
    // *signature text* and splitting the inside on `,` — so
    // `enum C = A | B`, Sail's bar-separated form, yielded no variants at
    // all and the assist never fired on it. The item tree records the
    // members; `fields` is what every other reader asks.
    for &id in item_tree.top_level_items() {
        if matches!(id.def_kind(item_tree), DefKind::Enum | DefKind::Union) {
            let members: Vec<String> =
                id.fields(item_tree).iter().map(|f| f.name.to_string()).collect();
            if !members.is_empty() {
                enum_name = Some(id.name(item_tree).as_str().to_string());
                all_variants = members;
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
                hir::Pat::Bind(name) => {
                    if name == "_" {
                        return None; // Wildcard covers everything
                    }
                    covered.push(name.clone());
                }
                hir::Pat::App { ctor, .. } => {
                    covered.push(ctor.clone());
                }
                hir::Pat::Wild => return None,
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
                         // Each new arm's body is a `todo`, which is the thing to type over —
                         // so for a client that can expand snippets they are the tabstops, the
                         // first one being where the cursor lands.
    let snippet = ctx.config.snippet_cap.is_some();
    let mut new_arms = String::new();
    for (i, variant) in missing.iter().enumerate() {
        let body = if snippet {
            format!("${{{}:todo(\"implement {variant}\")}}", i + 1)
        } else {
            format!("todo(\"implement {variant}\")")
        };
        new_arms.push_str(&format!("{indent}{variant} => {body},\n"));
    }

    // The arms go on their own lines, after a comma if the last existing
    // arm does not already end with one. Without this they were spliced
    // straight in front of the `}`: `match c { A => 1     B => todo(…),`,
    // which does not parse. Nothing noticed, because the assist could not
    // fire on a bar-separated enum at all.
    let before = text.get(..insert_offset)?.trim_end();
    let separator = if before.ends_with(['{', ',']) { "\n" } else { ",\n" };
    let new_arms = format!("{separator}{new_arms}");
    // Replacing the whitespace before the `}` rather than inserting at it,
    // so the last arm does not end up with a space before its comma.
    let range = base_db::text_range(before.len(), insert_offset);

    let label = format!("Fill {} missing match arms", missing.len());
    acc.add(
        AssistId("add_missing_match_arms", AssistKind::QuickFix),
        label,
        ctx.range,
        |builder| match ctx.config.snippet_cap {
            Some(_) => builder.replace_snippet(range, new_arms),
            None => builder.replace(range, new_arms),
        },
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
    use crate::tests::{check_assist, check_assist_with_snippets};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            add_missing_match_arms,
            "enum C = A | B\nval f : C -> int\nfunction f(c) = mat$0ch c { A => 1 }\n",
            expect![[r#"
                enum C = A | B
                val f : C -> int
                function f(c) = match c { A => 1,
                    B => todo("implement B"),
                }
            "#]],
        );
    }

    /// A client that can expand snippets lands in the first arm it has to
    /// write, and tabs to the next.
    #[test]
    fn leaves_the_cursor_in_the_first_new_arm() {
        check_assist_with_snippets(
            add_missing_match_arms,
            "enum C = A | B | D\nval f : C -> int\nfunction f(c) = mat$0ch c { A => 1 }\n",
            expect![[r#"
                enum C = A | B | D
                val f : C -> int
                function f(c) = match c { A => 1,
                    B => ${1:todo("implement B")},
                    D => ${2:todo("implement D")},
                }
            "#]],
        );
    }
}
