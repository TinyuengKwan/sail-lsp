//! `extract_type_alias` assist.
//! Extracts the selected type expression into a `type` alias.
//!
//! ```sail
//! val foo : bits(32) -> bits(32)
//! ```
//! → (select `bits(32)`)
//! ```sail
//! type Word = bits(32)
//! val foo : Word -> Word
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

/// Requires a non-empty selection covering a type expression.
/// Creates a `type Alias = selected_type` at the top of the current
/// definition, and replaces the original occurrence with `Alias`.
pub(crate) fn extract_type_alias(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    // Require non-empty selection
    if ctx.has_empty_selection() {
        return None;
    }

    let text = ctx.source_text();
    let range = ctx.selection_trimmed();

    // Extract the selected text — it should look like a type expression
    let selected = text.get(base_db::range_start(range)..base_db::range_end(range))?;
    let selected = selected.trim();

    if selected.is_empty() {
        return None;
    }

    // Basic validation: the selection should look like a type
    // (starts with a letter or `(` for tuple, `{` for record)
    let first_char = selected.chars().next()?;
    if !first_char.is_alphabetic() && first_char != '(' && first_char != '{' {
        return None;
    }

    // Collect type variables used in the selected type
    // Sail type vars look like 'a, 'b, etc.
    let type_vars = collect_type_vars(selected);

    // Build the type alias
    let alias_name = "NewType";
    let type_params =
        if type_vars.is_empty() { String::new() } else { format!("({})", type_vars.join(", ")) };
    let alias_usage = if type_vars.is_empty() {
        alias_name.to_string()
    } else {
        format!("{alias_name}{type_params}")
    };

    // Find the start of the current top-level item (function, val, type, etc.)
    // by searching backward for a line that starts with a keyword
    let item_start = find_item_start(text, base_db::range_start(range));

    let target = base_db::text_range(base_db::range_start(range), base_db::range_end(range));

    // Build text edits: insert alias definition + replace selected type
    let alias_def = format!("type {alias_name}{type_params} = {selected}\n\n");
    let edits = vec![
        // Insert the type alias definition before the current item
        TextEdit { range: base_db::text_range(item_start, item_start), new_text: alias_def },
        // Replace the selected type with the alias name
        TextEdit { range: target, new_text: alias_usage },
    ];

    acc.add_with_edits(
        AssistId("extract_type_alias", AssistKind::RefactorExtract),
        "Extract type as type alias",
        target,
        edits,
    );
    Some(())
}

/// Collect type variables ('a, 'b, etc.) from a type expression.
fn collect_type_vars(type_text: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut chars = type_text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\'' {
            // Collect the type variable name
            let mut name = String::from('\'');
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    name.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if name.len() > 1 && seen.insert(name.clone()) {
                vars.push(name);
            }
        }
    }
    vars
}

/// Find the start of the current top-level item by searching backward
/// for a line beginning with a keyword (function, val, type, etc.).
fn find_item_start(text: &str, offset: usize) -> usize {
    let keywords = [
        "function",
        "val",
        "type",
        "struct",
        "enum",
        "union",
        "register",
        "mapping",
        "overload",
        "let",
        "var",
        "scattered",
        "bitfield",
        "default",
    ];

    // Search backward line by line
    let before = &text[..offset];
    for (line_start, _) in before.rmatch_indices('\n') {
        let line_start = line_start + 1; // Skip the newline itself
        let line = &text[line_start..];
        let trimmed = line.trim_start();
        for kw in &keywords {
            if trimmed.starts_with(kw) {
                return line_start;
            }
        }
    }
    // Check the very first line
    let trimmed = text.trim_start();
    for kw in &keywords {
        if trimmed.starts_with(kw) {
            return 0;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_type_vars() {
        assert_eq!(collect_type_vars("bits('n)"), vec!["'n"]);
        assert_eq!(collect_type_vars("vector('n, 'a)"), vec!["'n", "'a"]);
        assert_eq!(collect_type_vars("int"), Vec::<String>::new());
    }
}
