//! Record literal field completion — suggests missing fields in struct literals.
//! Triggers when the cursor is inside a struct literal `{ field1 = ..., | }`
//! and suggests fields that haven't been filled yet.
//!
//! Also handles struct pattern completion: `match x { Foo { | } => ... }`

use crate::item::CompletionItem;
use ide_db::FileDb;

/// Complete record fields inside a struct literal or struct pattern.
/// # Arguments
/// - `file`: current file
/// - `text`: full source text
/// - `offset`: cursor byte offset
/// - `prefix`: text typed so far
/// - `all_files`: workspace files for looking up struct definitions
pub fn complete_record(
    file: &dyn FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
    all_files: &[(&url::Url, &dyn FileDb)],
) -> Vec<CompletionItem> {
    // 1. Check if we're inside a `{ ... }` block that looks like a struct literal
    let Some(context) = find_record_context(text, offset) else {
        return Vec::new();
    };

    // 2. Find the struct name and its field definitions
    let struct_name = &context.struct_name;
    let already_filled = &context.filled_fields;

    let mut items = Vec::new();
    let prefix_lower = prefix.to_ascii_lowercase();

    // 3. Look up struct fields across workspace
    for (_, ws_file) in all_files {
        if let Some(tree) = ws_file.item_tree() {
            for &id in tree.top_level_items() {
                if !ide_db::symbol_visible_from(file, *ws_file, id.name(tree).as_str()) {
                    continue;
                }
                if id.name(tree).as_str() == *struct_name
                    && matches!(id.def_kind(tree), hir::DefKind::Struct | hir::DefKind::Bitfield)
                {
                    // The fields the item tree recorded, not the ones a regex
                    // found in the definition's text.
                    for field in id.fields(tree).iter().map(|f| f.name.to_string()) {
                        // Skip already-filled fields
                        if already_filled.contains(&field) {
                            continue;
                        }
                        // Apply prefix filter
                        if !prefix_lower.is_empty()
                            && !field.to_ascii_lowercase().starts_with(&prefix_lower)
                        {
                            continue;
                        }
                        items.push(crate::render::render_record_field(&field, struct_name));
                    }
                }
            }
        }
    }

    items
}

/// Context for record literal/pattern completion.
struct RecordContext {
    /// Name of the struct being constructed.
    struct_name: String,
    /// Fields already filled in the literal.
    filled_fields: Vec<String>,
}

/// Detect if the cursor is inside a struct literal or pattern.
///
/// Looks backwards from offset for `StructName {` pattern and forwards
/// for the closing `}`.
fn find_record_context(text: &str, offset: usize) -> Option<RecordContext> {
    // Look backwards from cursor to find `{`
    let before = &text[..offset];
    let brace_pos = before.rfind('{')?;

    // Find the struct name before `{`
    let before_brace = before[..brace_pos].trim_end();
    let struct_name =
        before_brace.rsplit(|c: char| !c.is_alphanumeric() && c != '_').next()?.to_string();

    if struct_name.is_empty() {
        return None;
    }

    // Check it starts with uppercase (Sail struct convention)
    if !struct_name.chars().next()?.is_uppercase() {
        return None;
    }

    // Collect already-filled fields (look for `name =` patterns between { and cursor)
    let inside = &text[brace_pos + 1..offset];
    let filled_fields: Vec<String> = inside
        .split(',')
        .filter_map(|segment| {
            let trimmed = segment.trim();
            let eq_pos = trimmed.find('=')?;
            let field_name = trimmed[..eq_pos].trim().to_string();
            if field_name.is_empty() {
                None
            } else {
                Some(field_name)
            }
        })
        .collect();

    Some(RecordContext { struct_name, filled_fields })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_record_context_basic() {
        let text = "let p = Point { x = 1, ";
        let ctx = find_record_context(text, text.len());
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.struct_name, "Point");
        assert_eq!(ctx.filled_fields, vec!["x"]);
    }

    #[test]
    fn find_record_context_no_brace() {
        let text = "let x = foo(";
        let ctx = find_record_context(text, text.len());
        assert!(ctx.is_none());
    }

    #[test]
    fn find_record_context_lowercase_not_struct() {
        let text = "let x = { foo = 1, ";
        let ctx = find_record_context(text, text.len());
        // lowercase name not treated as struct
        assert!(ctx.is_none());
    }
}
