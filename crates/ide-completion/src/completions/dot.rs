//! Dot completion provider — field and method names after `.`.
//!
//! Two paths:
//! 1. Type-based: `receiver_ty` resolved via `Semantics::type_of_expr` →
//!    extract type name → scan workspace for struct/bitfield fields.
//! 2. Text-heuristic fallback: `binding_type_text` → same field scan.

use super::Completions;
use crate::context::{CompletionContext, DotAccess};
use crate::item::CompletionItem;
use ide_db::FileDb;
use url::Url;

/// Complete dot accesses (fields).
///
/// When `dot_access.receiver_ty` is available (resolved via Semantics),
/// extracts the type name and looks up struct/bitfield fields across the
/// workspace. Falls back to text-heuristic `field_completions()` otherwise.
pub(crate) fn complete_dot(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    dot_access: &DotAccess,
    all_files: &[(&Url, &dyn FileDb)],
) {
    // Type-based path: receiver_ty resolved by sema.type_of_expr.
    if let Some(ref receiver_ty) = dot_access.receiver_ty {
        let ty_name = receiver_ty.display_text();
        // Strip type parameters: "Struct_name(...)" → "Struct_name".
        let base_name = ty_name.split('(').next().unwrap_or(&ty_name).trim();
        if !base_name.is_empty() {
            let prefix_lower = ctx.prefix.to_ascii_lowercase();
            let mut found = false;
            for (_, file) in all_files {
                if let Some(tree) = file.item_tree() {
                    for &id in tree.top_level_items() {
                        if !ctx.can_see(*file, id.name(tree).as_str()) {
                            continue;
                        }
                        if id.name(tree).as_str() != base_name {
                            continue;
                        }
                        if !matches!(
                            id.def_kind(tree),
                            hir::DefKind::Struct | hir::DefKind::Bitfield
                        ) {
                            continue;
                        }
                        // The fields the item tree recorded, not the ones a
                        // regex found in the definition's text.
                        for field in id.fields(tree).iter().map(|f| f.name.to_string()) {
                            if !prefix_lower.is_empty()
                                && !field.to_ascii_lowercase().starts_with(&prefix_lower)
                            {
                                continue;
                            }
                            acc.add(crate::render::render_field(&field, None, base_name));
                            found = true;
                        }
                    }
                }
            }
            if found {
                return;
            }
        }
    }

    // Text-heuristic fallback.
    let items = field_completions(all_files, ctx.file, ctx.text, ctx.offset, ctx.prefix);
    acc.add_many(items);
}

/// Field completion after `.`: suggest struct field names when the receiver
/// type is a known struct/record. Also suggests bitfield field names.
pub(crate) fn field_completions(
    files: &[(&url::Url, &dyn ide_db::FileDb)],
    current_file: &dyn ide_db::FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let prefix_start = offset.saturating_sub(prefix.len());
    if prefix_start == 0 {
        return Vec::new();
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return Vec::new();
    }

    // Extract receiver name (identifier before the dot)
    let dot_pos = prefix_start - 1;
    let receiver = extract_receiver_expr(text, dot_pos);
    if receiver.is_empty() {
        return Vec::new();
    }

    let prefix_lower = prefix.to_ascii_lowercase();
    let mut items = Vec::new();

    // Look up the receiver's type from binding_type_text
    let receiver_span = parser::Span::new(dot_pos - receiver.len(), dot_pos);
    let ty_text = current_file
        .binding_type_text(receiver_span)
        .or_else(|| current_file.cached_expr_type_text(receiver_span));

    if let Some(ref ty_name) = ty_text {
        // Search all files for struct/record/bitfield with matching type name
        for (_, file) in files {
            if let Some(tree) = file.item_tree() {
                // The fields the item tree recorded, rather than the field
                // names a regex found in the definition's text.
                for &id in tree.top_level_items() {
                    if !ide_db::symbol_visible_from(current_file, *file, id.name(tree).as_str()) {
                        continue;
                    }
                    let decl_name = id.name(tree).to_string();
                    if (decl_name == *ty_name || ty_name.starts_with(&decl_name))
                        && matches!(
                            id.def_kind(tree),
                            hir::DefKind::Struct | hir::DefKind::Bitfield
                        )
                    {
                        for field in id.fields(tree).iter().map(|f| f.name.to_string()) {
                            if !prefix_lower.is_empty()
                                && !field.to_ascii_lowercase().starts_with(&prefix_lower)
                            {
                                continue;
                            }
                            items.push(crate::render::render_field(&field, None, &decl_name));
                        }
                    }
                }
            }
        }
    }

    items
}

/// Extract the receiver expression text before a dot.
pub(crate) fn extract_receiver_expr(text: &str, dot_pos: usize) -> &str {
    let bytes = text.as_bytes();
    let mut end = dot_pos;
    // Skip whitespace before the dot
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return "";
    }

    // Handle closing brackets by finding the matching open
    let last_byte = bytes[end - 1];
    if last_byte == b')' || last_byte == b']' {
        let open = if last_byte == b')' { b'(' } else { b'[' };
        let mut depth = 1i32;
        let mut pos = end - 2;
        loop {
            if bytes[pos] == last_byte {
                depth += 1;
            } else if bytes[pos] == open {
                depth -= 1;
                if depth == 0 {
                    // Now also grab the identifier before the open paren
                    let mut start = pos;
                    while start > 0
                        && (bytes[start - 1].is_ascii_alphanumeric()
                            || bytes[start - 1] == b'_'
                            || bytes[start - 1] == b'?')
                    {
                        start -= 1;
                    }
                    return &text[start..end];
                }
            }
            if pos == 0 {
                break;
            }
            pos -= 1;
        }
        return "";
    }

    // Otherwise, grab an identifier
    let mut start = end;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric()
            || bytes[start - 1] == b'_'
            || bytes[start - 1] == b'?'
            || bytes[start - 1] == b'\'')
    {
        start -= 1;
    }
    &text[start..end]
}
