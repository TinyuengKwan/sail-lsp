//! Expression completion — values, functions, keywords in expression position.
//! RA entry: `complete_expr_path(acc, ctx, path_ctx, expr_ctx)`
//! dispatches on `Qualified::No` vs `Qualified::With`.
//!
//! Sail: handles both qualified and unqualified completion inline,
//! matching RA's single-module pattern.

use super::Completions;
use ide_db::ide_types::{
    CompletionItem, CompletionItemKind, CompletionRelevance, CompletionRelevanceFn,
    CompletionRelevanceReturnType, CompletionRelevanceTypeMatch,
};
use ide_db::FileDb;
use url::Url;

use crate::context::{
    self, CompletionContext, CompletionPosition, ExprCtx, PathCompletionCtx, Qualified,
};

/// Complete expression paths — the main expression-position provider.
/// Dispatches on qualification:
/// - `Qualified::No` → locals + scope items + keywords
/// - `Qualified::With` → qualified member access
pub(crate) fn complete_expr_path(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
    path_ctx: &PathCompletionCtx,
    _expr_ctx: &ExprCtx,
    keywords: &[&str],
    builtins: &[&str],
) {
    match &path_ctx.qualified {
        Qualified::No => {
            // Unqualified: all visible names + keywords
            complete_unqualified(acc, ctx, all_files, builtins);
            super::keyword::complete_keywords(acc, ctx, keywords);
        }
        Qualified::With { .. } => {
            // Qualified: enum/struct members via TypeName.
            complete_qualified(ctx, acc, all_files);
        }
    }
}

/// Complete members of a qualified type name (`TypeName.|`).
///
/// calls `acc.add_enum_variants()` for enum types.
fn complete_qualified(
    ctx: &CompletionContext<'_>,
    acc: &mut Completions,
    all_files: &[(&Url, &dyn FileDb)],
) {
    let Qualified::With { ref path } = ctx.qualified else {
        return;
    };

    let prefix_lower = ctx.prefix_lower();
    let path_lower = path.to_ascii_lowercase();
    let mut seen = std::collections::HashSet::new();

    for (_uri, file) in all_files {
        let Some(parsed) = file.parsed() else {
            continue;
        };

        for decl in &parsed.decls {
            if decl.scope != syntax::parser_lower::Scope::TopLevel {
                continue;
            }

            // Match enum type → emit its members
            if matches!(decl.kind, syntax::parser_lower::DeclKind::Enum)
                && decl.name.to_ascii_lowercase() == path_lower
            {
                // Found the enum — now emit its members
                emit_enum_members(&decl.name, parsed, &prefix_lower, &mut seen, acc);
            }

            // Match union type → emit its constructors
            if matches!(decl.kind, syntax::parser_lower::DeclKind::Union)
                && decl.name.to_ascii_lowercase() == path_lower
            {
                emit_union_constructors(
                    &decl.name,
                    parsed,
                    file.text(),
                    decl.span,
                    &prefix_lower,
                    &mut seen,
                    acc,
                );
            }

            // Match struct type → emit fields (supplements field.rs
            // for the uppercase-name case)
            if matches!(
                decl.kind,
                syntax::parser_lower::DeclKind::Struct | syntax::parser_lower::DeclKind::Bitfield
            ) && decl.name.to_ascii_lowercase() == path_lower
            {
                emit_struct_fields(
                    &decl.name,
                    file.text(),
                    decl.span,
                    &prefix_lower,
                    &mut seen,
                    acc,
                );
            }
        }
    }
}

/// Emit enum members that belong to a given enum type.
fn emit_enum_members(
    enum_name: &str,
    parsed: &syntax::parser_lower::ParsedFile,
    prefix_lower: &str,
    seen: &mut std::collections::HashSet<String>,
    acc: &mut Completions,
) {
    // Enum members are emitted as top-level DeclKind::EnumMember
    // declarations immediately after the enum definition.
    // We collect all EnumMember decls whose parent enum matches.
    //
    // Heuristic: members declared between this enum and the next
    // non-member decl belong to this enum.
    let mut in_enum = false;
    for decl in &parsed.decls {
        if decl.scope != syntax::parser_lower::Scope::TopLevel {
            continue;
        }
        if decl.name == enum_name && matches!(decl.kind, syntax::parser_lower::DeclKind::Enum) {
            in_enum = true;
            continue;
        }
        if in_enum {
            if !matches!(decl.kind, syntax::parser_lower::DeclKind::EnumMember) {
                break; // past the enum's members
            }
            if !prefix_lower.is_empty() && !decl.name.to_ascii_lowercase().starts_with(prefix_lower)
            {
                continue;
            }
            if !seen.insert(decl.name.clone()) {
                continue;
            }
            acc.add(CompletionItem {
                label: decl.name.clone(),
                kind: CompletionItemKind::EnumMember,
                detail: Some(format!("member of {enum_name}")),
                documentation: None,
                insert_text: Some(decl.name.clone()),
                text_edit: None,
                sort_text: None,
                filter_text: Some(decl.name.clone()),
                deprecated: false,
                relevance: CompletionRelevance {
                    exact_name_match: false,
                    is_local: false,
                    type_match: Some(ide_db::ide_types::CompletionRelevanceTypeMatch::Exact),
                    ..Default::default()
                },
            });
        }
    }
}

/// Emit union constructors for a given union type.
fn emit_union_constructors(
    union_name: &str,
    parsed: &syntax::parser_lower::ParsedFile,
    text: &str,
    span: parser::Span,
    prefix_lower: &str,
    seen: &mut std::collections::HashSet<String>,
    acc: &mut Completions,
) {
    // Extract constructors from the union definition body
    let def_text = text.get(span.start..span.end).unwrap_or("");
    let Some(brace_start) = def_text.find('{') else {
        return;
    };
    let Some(brace_end) = def_text.rfind('}') else {
        return;
    };
    if brace_start >= brace_end {
        return;
    }
    let inner = &def_text[brace_start + 1..brace_end];
    for part in inner.split(',') {
        let part = part.trim();
        // Union variants look like `CtorName : type`
        let ctor_name = if let Some(colon_pos) = part.find(':') {
            part[..colon_pos].trim()
        } else {
            part.trim()
        };
        if ctor_name.is_empty() {
            continue;
        }
        if !prefix_lower.is_empty() && !ctor_name.to_ascii_lowercase().starts_with(prefix_lower) {
            continue;
        }
        if !seen.insert(ctor_name.to_string()) {
            continue;
        }
        acc.add(CompletionItem {
            label: ctor_name.to_string(),
            kind: CompletionItemKind::EnumMember,
            detail: Some(format!("constructor of {union_name}")),
            documentation: None,
            insert_text: Some(format!("{ctor_name}($0)")),
            text_edit: None,
            sort_text: None,
            filter_text: Some(ctor_name.to_string()),
            deprecated: false,
            relevance: CompletionRelevance {
                type_match: Some(ide_db::ide_types::CompletionRelevanceTypeMatch::Exact),
                ..Default::default()
            },
        });
    }

    // Also check union_constructor_names for cross-file constructors
    for ctor in &parsed.union_constructor_names {
        if !prefix_lower.is_empty() && !ctor.to_ascii_lowercase().starts_with(prefix_lower) {
            continue;
        }
        if !seen.insert(ctor.clone()) {
            continue;
        }
        acc.add(CompletionItem {
            label: ctor.clone(),
            kind: CompletionItemKind::EnumMember,
            detail: Some(format!("constructor of {union_name}")),
            documentation: None,
            insert_text: Some(format!("{ctor}($0)")),
            text_edit: None,
            sort_text: None,
            filter_text: Some(ctor.clone()),
            deprecated: false,
            relevance: CompletionRelevance {
                type_match: Some(ide_db::ide_types::CompletionRelevanceTypeMatch::Exact),
                ..Default::default()
            },
        });
    }
}

/// Emit struct/bitfield fields for a given struct type.
fn emit_struct_fields(
    struct_name: &str,
    text: &str,
    span: parser::Span,
    prefix_lower: &str,
    seen: &mut std::collections::HashSet<String>,
    acc: &mut Completions,
) {
    let def_text = text.get(span.start..span.end).unwrap_or("");
    let fields = crate::extract_struct_fields(def_text);
    for field in fields {
        if !prefix_lower.is_empty() && !field.to_ascii_lowercase().starts_with(prefix_lower) {
            continue;
        }
        if !seen.insert(field.clone()) {
            continue;
        }
        acc.add(CompletionItem {
            label: field.clone(),
            kind: CompletionItemKind::Field,
            detail: Some(format!("field of {struct_name}")),
            documentation: None,
            insert_text: Some(field),
            text_edit: None,
            sort_text: None,
            filter_text: None,
            deprecated: false,
            relevance: CompletionRelevance {
                type_match: Some(ide_db::ide_types::CompletionRelevanceTypeMatch::Exact),
                ..Default::default()
            },
        });
    }
}

/// Complete unqualified names (top-level definitions, builtins, local bindings).
fn complete_unqualified(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
    builtins: &[&str],
) {
    let prefix_lower = ctx.prefix_lower();

    // 投産-3: Use Resolver::names_in_scope() to collect scope-aware names.
    // This supplements the ParsedFile-based iteration below with names
    // from the DefMap scope chain (local bindings, block scope, workspace).
    if let Some(file_text) = ctx.file_text {
        {
            let scope_names = ctx.sema.names_in_scope(file_text, ctx.offset);
            for name in &scope_names {
                let name_str: &str = name.as_str();
                if !prefix_lower.is_empty()
                    && !name_str.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                // Scope-aware names get high relevance (is_local = true)
                acc.add(ide_db::ide_types::CompletionItem {
                    label: name_str.to_string(),
                    kind: ide_db::ide_types::CompletionItemKind::Variable,
                    detail: Some("scope".to_string()),
                    documentation: None,
                    insert_text: Some(name_str.to_string()),
                    text_edit: None,
                    sort_text: None,
                    filter_text: Some(name_str.to_string()),
                    deprecated: false,
                    relevance: CompletionRelevance {
                        is_local: true,
                        ..Default::default()
                    },
                });
            }
        }
    }

    // Builtins
    for builtin in builtins {
        if !prefix_lower.is_empty() && !builtin.to_ascii_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let kind = if builtin.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            CompletionItemKind::Struct
        } else {
            CompletionItemKind::Constant
        };
        acc.add(CompletionItem {
            label: builtin.to_string(),
            kind,
            detail: Some("builtin".to_string()),
            documentation: None,
            insert_text: None,
            text_edit: None,
            sort_text: None,
            filter_text: Some(builtin.to_string()),
            deprecated: false,
            relevance: Default::default(),
        });
    }

    // Workspace symbols from all files
    let mut seen = std::collections::HashSet::new();
    for (_candidate_uri, candidate_file) in all_files {
        if let Some(parsed) = candidate_file.parsed() {
            for decl in &parsed.decls {
                if decl.scope != syntax::parser_lower::Scope::TopLevel {
                    continue;
                }
                if !prefix_lower.is_empty()
                    && !decl.name.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                // Filter by context: type position → only types
                let (kind, detail) = decl_kind_to_completion(decl.kind);
                if ctx.position == CompletionPosition::TypeAnnotation {
                    if !matches!(
                        kind,
                        CompletionItemKind::Struct
                            | CompletionItemKind::Enum
                            | CompletionItemKind::TypeParameter
                    ) {
                        continue;
                    }
                }
                if !seen.insert(decl.name.clone()) {
                    continue;
                }
                // Function snippet + relevance from signature index
                let sig_info = if matches!(kind, CompletionItemKind::Function) {
                    candidate_file.signature_index().and_then(|idx| idx.get(&decl.name))
                } else {
                    None
                };
                let snippet = sig_info.map(|sig| ide_db::function_snippet(&decl.name, &sig.params));
                // Populate CompletionRelevanceFn for functions + type_match.
                let type_match = compute_fn_type_match(ctx, sig_info);
                let relevance = if let Some(sig) = sig_info {
                    CompletionRelevance {
                        function: Some(CompletionRelevanceFn {
                            has_params: !sig.params.is_empty(),
                            has_self_param: false, // Sail has no self params
                            return_type: CompletionRelevanceReturnType::Other,
                        }),
                        type_match,
                        exact_name_match: ctx.expected_name.as_deref() == Some(&decl.name),
                        ..Default::default()
                    }
                } else {
                    CompletionRelevance {
                        exact_name_match: ctx.expected_name.as_deref() == Some(&decl.name),
                        ..Default::default()
                    }
                };
                acc.add(CompletionItem {
                    label: decl.name.clone(),
                    kind,
                    detail: Some(detail),
                    documentation: None,
                    insert_text: Some(snippet.unwrap_or(decl.name.clone())),
                    text_edit: None,
                    sort_text: None,
                    filter_text: Some(decl.name.clone()),
                    deprecated: false,
                    relevance,
                });
            }

            // Local bindings from current file handled via symbol_occurrences
            // (skipped here — would need current_uri comparison)
        }
    }
}

fn decl_kind_to_completion(kind: syntax::parser_lower::DeclKind) -> (CompletionItemKind, String) {
    use syntax::parser_lower::DeclKind;
    match kind {
        DeclKind::Function => (CompletionItemKind::Function, "function".into()),
        DeclKind::Value => (CompletionItemKind::Function, "value specification".into()),
        DeclKind::Mapping => (CompletionItemKind::Function, "mapping".into()),
        DeclKind::Overload => (CompletionItemKind::Function, "overload".into()),
        DeclKind::Type
        | DeclKind::Struct
        | DeclKind::Union
        | DeclKind::Bitfield
        | DeclKind::Newtype => (CompletionItemKind::Struct, "type".into()),
        DeclKind::Enum => (CompletionItemKind::Enum, "enum".into()),
        DeclKind::Register => (CompletionItemKind::Variable, "register".into()),
        DeclKind::EnumMember => (CompletionItemKind::EnumMember, "enum member".into()),
        DeclKind::Let => (CompletionItemKind::Variable, "let binding".into()),
        DeclKind::Var => (CompletionItemKind::Variable, "var binding".into()),
        DeclKind::Parameter => (CompletionItemKind::Variable, "parameter".into()),
    }
}

/// Compute type_match for a function candidate by comparing its return type
/// with the expected type in the completion context.
fn compute_fn_type_match(
    ctx: &CompletionContext<'_>,
    sig_info: Option<&ide_db::CallableSignature>,
) -> Option<CompletionRelevanceTypeMatch> {
    let expected = ctx.expected_type.as_ref()?;
    let sig = sig_info?;
    let ret_str = sig.return_type.as_deref()?;
    if ret_str.is_empty() {
        return None;
    }
    let candidate_ty = hir_ty::infer::Ty::named(ret_str);
    context::compute_type_match(expected, &candidate_ty)
}
