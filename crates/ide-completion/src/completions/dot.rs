//! Dot completion provider — field and method names after `.`.
//!
//! Two paths:
//! 1. Type-based: `receiver_ty` resolved via `Semantics::type_of_expr` →
//!    extract type name → scan workspace for struct/bitfield fields.
//! 2. Text-heuristic fallback: `binding_type_text` → same field scan.

use super::Completions;
use crate::context::{CompletionContext, DotAccess};
use crate::CompletionItemKind;
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
                if let Some(parsed) = file.parsed() {
                    for decl in &parsed.decls {
                        if decl.name != base_name {
                            continue;
                        }
                        if !matches!(
                            decl.kind,
                            syntax::parser_lower::DeclKind::Struct
                                | syntax::parser_lower::DeclKind::Bitfield
                        ) {
                            continue;
                        }
                        let def_text =
                            file.text().get(decl.span.start..decl.span.end).unwrap_or("");
                        for field in crate::extract_struct_fields(def_text) {
                            if !prefix_lower.is_empty()
                                && !field.to_ascii_lowercase().starts_with(&prefix_lower)
                            {
                                continue;
                            }
                            acc.add(crate::IdeDbCompletionItem {
                                label: field.clone(),
                                kind: CompletionItemKind::Field,
                                detail: Some(format!("field of {}", decl.name)),
                                documentation: None,
                                insert_text: None,
                                text_edit: None,
                                sort_text: Some(format!("0{field}")),
                                filter_text: None,
                                deprecated: false,
                                relevance: Default::default(),
                            });
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
    let items = crate::field_completions(all_files, ctx.file, ctx.text, ctx.offset, ctx.prefix);
    acc.add_many(items);
}
