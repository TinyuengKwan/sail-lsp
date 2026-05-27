//! `merge_match_arms` assist.
//! Merges consecutive match arms that have the same body expression
//! into a single arm with an OR pattern:
//!
//! ```sail
//! // Before:
//! match x {
//!     A => expr,
//!     B => expr,
//!     C => other,
//! }
//!
//! // After:
//! match x {
//!     A | B => expr,
//!     C => other,
//! }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

/// Merge consecutive match arms with identical bodies.
pub(crate) fn merge_match_arms(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let text = ctx.file.text();
    let offset = ctx.offset();

    let (cst_root, _) = syntax::parse_text(text);

    // Find MATCH_EXPR at cursor.
    let match_node = cst_root.descendants().find(|node| {
        node.kind() == SK::MATCH_EXPR && {
            let range = node.text_range();
            usize::from(range.start()) <= offset && offset <= usize::from(range.end())
        }
    })?;

    // Parse arms from the match expression text.
    let match_text = match_node.text().to_string();
    let body_start = match_text.find('{')?;
    let body_end = match_text.rfind('}')?;
    let arms_text = &match_text[body_start + 1..body_end];

    // Parse arms: split by `,` (simplified — doesn't handle nested commas)
    let raw_arms: Vec<&str> =
        arms_text.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

    if raw_arms.len() < 2 {
        return None;
    }

    // Parse each arm into (pattern, body)
    let parsed_arms: Vec<(&str, &str)> = raw_arms
        .iter()
        .filter_map(|arm| {
            let arrow_pos = arm.find("=>")?;
            let pat = arm[..arrow_pos].trim();
            let body = arm[arrow_pos + 2..].trim();
            Some((pat, body))
        })
        .collect();

    if parsed_arms.len() < 2 {
        return None;
    }

    // Find groups of consecutive arms with the same body.
    let mut groups: Vec<(Vec<&str>, &str)> = Vec::new();
    for (pat, body) in &parsed_arms {
        if let Some(last) = groups.last_mut() {
            if last.1 == *body {
                last.0.push(pat);
                continue;
            }
        }
        groups.push((vec![pat], body));
    }

    // Check if any group has more than one pattern (something to merge).
    if groups.iter().all(|(pats, _)| pats.len() == 1) {
        return None;
    }

    // Build merged arms text.
    let indent = "    ";
    let merged: String = groups
        .iter()
        .map(|(pats, body)| {
            let combined_pat = pats.join(" | ");
            format!("{indent}{combined_pat} => {body},\n")
        })
        .collect();

    // Replace the match body.
    let match_start = usize::from(match_node.text_range().start());
    let insert_start = match_start + body_start + 1;
    let insert_end = match_start + body_end;

    let edit = TextEdit {
        range: base_db::text_range(insert_start, insert_end),
        new_text: format!("\n{merged}"),
    };

    acc.add_with_edits(
        AssistId("merge_match_arms", AssistKind::RefactorRewrite),
        "Merge match arms with same body",
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
        let labels = check_assist(merge_match_arms, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
