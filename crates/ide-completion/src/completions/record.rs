//! Record literal field completion — suggests missing fields in struct literals.
//! Triggers when the cursor is inside a struct literal `{ field1 = ..., | }`
//! and suggests fields that haven't been filled yet.
//!
//! Also handles struct pattern completion: `match x { Foo { | } => ... }`

use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::{CompletionItem, CompletionRelevance};
use ide_db::FileDb;

/// Complete record fields inside a struct literal or struct pattern.
/// # Arguments
/// - `file`: current file
/// - `text`: full source text
/// - `offset`: cursor byte offset
/// - `prefix`: text typed so far
/// - `all_files`: workspace files for looking up struct definitions
pub fn complete_record(
    _file: &dyn FileDb,
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
        if let Some(parsed) = ws_file.parsed() {
            for decl in &parsed.decls {
                if decl.name == *struct_name
                    && matches!(
                        decl.kind,
                        syntax::parser_lower::DeclKind::Struct
                            | syntax::parser_lower::DeclKind::Bitfield
                    ) {
                        let def_text =
                            ws_file.text().get(decl.span.start..decl.span.end).unwrap_or("");
                        for field in extract_record_fields(def_text) {
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
                            items.push(CompletionItem {
                                label: field.clone(),
                                kind: CompletionItemKind::Field,
                                detail: Some(format!("field of {struct_name}")),
                                insert_text: Some(format!("{field} = ")),
                                text_edit: None,
                                sort_text: Some(format!("0{field}")),
                                filter_text: None,
                                documentation: None,
                                deprecated: false,
                                relevance: CompletionRelevance::default(),
                            });
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

/// Extract field names from a struct definition text.
///
/// Parses `struct Foo = { field1 : type1, field2 : type2 }` and
/// returns `["field1", "field2"]`.
fn extract_record_fields(def_text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    // Find the part between { and }
    let brace_start = match def_text.find('{') {
        Some(pos) => pos + 1,
        None => return fields,
    };
    let brace_end = def_text.rfind('}').unwrap_or(def_text.len());
    let inner = &def_text[brace_start..brace_end];

    for segment in inner.split(',') {
        let trimmed = segment.trim();
        // Field format: `name : type`
        if let Some(colon_pos) = trimmed.find(':') {
            let field_name = trimmed[..colon_pos].trim();
            if !field_name.is_empty()
                && field_name.chars().next().is_some_and(|c| c.is_alphabetic())
            {
                fields.push(field_name.to_string());
            }
        }
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_fields_from_struct() {
        let def = "struct Point = { x : int, y : int }";
        let fields = extract_record_fields(def);
        assert_eq!(fields, vec!["x", "y"]);
    }

    #[test]
    fn extract_fields_multiline() {
        let def = "struct Config = {\n  width : int,\n  height : int,\n  name : string\n}";
        let fields = extract_record_fields(def);
        assert_eq!(fields, vec!["width", "height", "name"]);
    }

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
