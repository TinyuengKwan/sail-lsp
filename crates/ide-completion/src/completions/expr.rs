//! Expression completion — values, functions, keywords in expression position.
//! RA entry: `complete_expr_path(acc, ctx, path_ctx, expr_ctx)`
//! dispatches on `Qualified::No` vs `Qualified::With`.
//!
//! Sail: handles both qualified and unqualified completion inline,
//! matching RA's single-module pattern.

use super::Completions;
use crate::item::{
    CompletionItem, CompletionRelevance, CompletionRelevanceFn, CompletionRelevanceReturnType,
    CompletionRelevanceTypeMatch,
};
use ide_db::defs::CompletionItemKind;
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
        let Some(tree) = file.item_tree() else {
            continue;
        };
        for &id in tree.top_level_items() {
            let name = id.name(tree).to_string();
            let scattered_clause = id.def_kind(tree) == hir::DefKind::ScatteredClause
                && id.scattered_type_kind(tree).is_some();
            if !scattered_clause && !ctx.can_see_item(*file, tree, id) {
                continue;
            }
            let member_kind = match id.scattered_type_kind(tree) {
                Some(hir::TypeDefShape::Enum) => hir::DefKind::Enum,
                Some(hir::TypeDefShape::Union) => hir::DefKind::Union,
                _ => id.def_kind(tree),
            };
            if name.to_ascii_lowercase() != path_lower {
                continue;
            }
            // The members are the definition's own fields — no heuristic
            // about which declarations follow which. `emit_enum_members`
            // used to decide that "members declared between this enum and
            // the next non-member declaration belong to it", and
            // `emit_union_constructors` sliced the braces out of the
            // definition's source text and split on commas.
            let members = || {
                id.fields(tree)
                    .iter()
                    .filter(|f| {
                        !matches!(member_kind, hir::DefKind::Enum | hir::DefKind::Union)
                            || ctx.can_see_member(*file, f)
                    })
                    .map(|f| f.name.to_string())
            };
            match member_kind {
                hir::DefKind::Enum => {
                    for member in members() {
                        emit(
                            acc,
                            &mut seen,
                            &prefix_lower,
                            &member,
                            CompletionItemKind::EnumMember,
                            format!("member of {name}"),
                            member.clone(),
                        );
                    }
                }
                hir::DefKind::Union => {
                    for ctor in members() {
                        let insert = format!("{ctor}($0)");
                        emit(
                            acc,
                            &mut seen,
                            &prefix_lower,
                            &ctor,
                            CompletionItemKind::EnumMember,
                            format!("constructor of {name}"),
                            insert,
                        );
                    }
                }
                hir::DefKind::Struct | hir::DefKind::Bitfield => {
                    for field in members() {
                        emit(
                            acc,
                            &mut seen,
                            &prefix_lower,
                            &field,
                            CompletionItemKind::Field,
                            format!("field of {name}"),
                            field.clone(),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Add one member completion, unless it is filtered out or already offered.
#[allow(clippy::too_many_arguments)]
fn emit(
    acc: &mut Completions,
    seen: &mut std::collections::HashSet<String>,
    prefix_lower: &str,
    label: &str,
    kind: CompletionItemKind,
    detail: String,
    insert_text: String,
) {
    if !prefix_lower.is_empty() && !label.to_ascii_lowercase().starts_with(prefix_lower) {
        return;
    }
    if !seen.insert(label.to_string()) {
        return;
    }
    acc.add(
        CompletionItem::new(kind, label)
            .detail(detail)
            .insert_text(insert_text)
            .filter_text(label)
            .relevance(CompletionRelevance {
                type_match: Some(crate::item::CompletionRelevanceTypeMatch::Exact),
                ..Default::default()
            }),
    );
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
            // The cursor may sit exactly at the exclusive end of a body.
            // Resolve at the identifier being completed, which is still inside
            // that body's scope, rather than losing all local candidates.
            let scope_offset = ctx.offset.saturating_sub(ctx.prefix.len());
            let scope_names = if hir::source_order::declarations_for(ctx.db, file_text).is_some() {
                ctx.sema.local_names_in_scope(file_text, scope_offset)
            } else {
                ctx.sema.names_in_scope(file_text, scope_offset)
            };
            for name in &scope_names {
                let name_str: &str = name.as_str();
                if !prefix_lower.is_empty()
                    && !name_str.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                // Scope-aware names get high relevance (is_local = true)
                acc.add(
                    crate::item::CompletionItem::new(
                        ide_db::defs::CompletionItemKind::Variable,
                        name_str,
                    )
                    .detail("scope")
                    .insert_text(name_str)
                    .filter_text(name_str)
                    .relevance(CompletionRelevance { is_local: true, ..Default::default() }),
                );
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
        acc.add(CompletionItem::new(kind, *builtin).detail("builtin").filter_text(*builtin));
    }

    // Workspace symbols from all files
    let mut seen = std::collections::HashSet::new();
    for (_candidate_uri, candidate_file) in all_files {
        if let Some(tree) = candidate_file.item_tree() {
            // An enum's members and a union's constructors are written
            // unqualified in Sail — `Red`, not `Color::Red` — so they belong
            // in this list beside the top-level names. They were only
            // offered after typing the enum's own name, which is a Rust
            // shape: the older engine this file replaced did offer them, and
            // the only test that noticed was calling that engine.
            if ctx.position != CompletionPosition::TypeAnnotation {
                for &id in tree.top_level_items() {
                    let owner = id.name(tree).to_string();
                    let (kind, what) = match id.def_kind(tree) {
                        hir::DefKind::Enum => (CompletionItemKind::EnumMember, "member"),
                        hir::DefKind::Union | hir::DefKind::Newtype => {
                            (CompletionItemKind::EnumMember, "constructor")
                        }
                        hir::DefKind::ScatteredClause if id.scattered_type_kind(tree).is_some() => {
                            (
                                CompletionItemKind::EnumMember,
                                if id.scattered_type_kind(tree) == Some(hir::TypeDefShape::Enum) {
                                    "member"
                                } else {
                                    "constructor"
                                },
                            )
                        }
                        _ => continue,
                    };
                    for field in id.fields(tree) {
                        let member = field.name.to_string();
                        if !prefix_lower.is_empty()
                            && !member.to_ascii_lowercase().starts_with(&prefix_lower)
                        {
                            continue;
                        }
                        if !ctx.can_see_member(*candidate_file, field) {
                            continue;
                        }
                        if !seen.insert((member.clone(), false)) {
                            continue;
                        }
                        acc.add(
                            CompletionItem::new(kind, member.clone())
                                .detail(format!("{what} of {owner}"))
                                .filter_text(member),
                        );
                    }
                }
            }
            for &id in tree.top_level_items() {
                let decl = DeclLike { name: id.name(tree).to_string() };
                if !prefix_lower.is_empty()
                    && !decl.name.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                // Prefix rejection is cheap and independent of visibility.
                // Resolve only candidates that can actually enter the list.
                if !ctx.can_see_item(*candidate_file, tree, id) {
                    continue;
                }
                // Filter by context: type position → only types
                let selected_kind = if id.def_kind(tree) == hir::DefKind::ScatteredHead
                    && id.scattered_type_kind(tree).is_some()
                {
                    hir::DefKind::Enum
                } else {
                    id.def_kind(tree)
                };
                let Some((kind, detail)) = def_kind_to_completion(selected_kind) else {
                    continue;
                };
                if ctx.position == CompletionPosition::TypeAnnotation
                    && !matches!(
                        kind,
                        CompletionItemKind::Struct
                            | CompletionItemKind::Enum
                            | CompletionItemKind::TypeParameter
                    )
                {
                    continue;
                }
                if !seen.insert((
                    decl.name.clone(),
                    matches!(
                        kind,
                        CompletionItemKind::Struct
                            | CompletionItemKind::Enum
                            | CompletionItemKind::TypeParameter
                    ),
                )) {
                    continue;
                }
                // Function snippet + relevance from signature index
                let selected_sigs = ctx.file_text.and_then(|ft| {
                    ide_db::signatures::callable_signatures_at(ctx.db, ft, ctx.offset, &decl.name)
                });
                let signature_index = (matches!(kind, CompletionItemKind::Function)
                    && selected_sigs.is_none())
                .then(|| ide_db::signatures::signature_index(*candidate_file));
                let sig_info = if matches!(kind, CompletionItemKind::Function) {
                    match &selected_sigs {
                        Some(sigs) => sigs.first(),
                        None => signature_index.as_ref().and_then(|index| index.get(&decl.name)),
                    }
                } else {
                    None
                };
                // Completing the name of an existing call must preserve its
                // argument list, including when the cursor is inside the name.
                let suffix = ctx.text[ctx.offset..]
                    .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '\'');
                let has_parens = suffix.trim_start().starts_with('(');
                let snippet = ctx
                    .config
                    .callable
                    .filter(|_| !has_parens)
                    .and_then(|_| sig_info)
                    .map(|sig| ide_db::function_snippet(&decl.name, &sig.params));
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
                acc.add(
                    CompletionItem::new(kind, decl.name.clone())
                        .detail(if selected_sigs.is_some() || ctx.config.full_function_signatures {
                            sig_info.map(|s| s.label.clone()).unwrap_or(detail)
                        } else {
                            detail
                        })
                        .insert_text(snippet.unwrap_or_else(|| decl.name.clone()))
                        .filter_text(decl.name.clone())
                        .relevance(relevance),
                );
            }

            // Local bindings from current file handled via symbol_occurrences
            // (skipped here — would need current_uri comparison)
        }
    }
}

/// A name from the item tree, in the shape the loop below already used.
struct DeclLike {
    name: String,
}

/// How a definition of this kind is offered, or `None` if it is not offered.
fn def_kind_to_completion(kind: hir::DefKind) -> Option<(CompletionItemKind, String)> {
    use hir::DefKind as D;
    Some(match kind {
        D::Function => (CompletionItemKind::Function, "function".into()),
        D::ValSpec => (CompletionItemKind::Function, "value specification".into()),
        D::Mapping | D::MappingSpec => (CompletionItemKind::Function, "mapping".into()),
        D::Outcome => (CompletionItemKind::Function, "outcome".into()),
        D::Overload => (CompletionItemKind::Function, "overload".into()),
        D::TypeAlias | D::Struct | D::Union | D::Bitfield | D::Newtype => {
            (CompletionItemKind::Struct, "type".into())
        }
        D::Enum => (CompletionItemKind::Enum, "enum".into()),
        D::Register => (CompletionItemKind::Variable, "register".into()),
        D::Let => (CompletionItemKind::Variable, "let binding".into()),
        D::Var => (CompletionItemKind::Variable, "var binding".into()),
        D::ScatteredHead => (CompletionItemKind::Function, "scattered definition".into()),
        D::ScatteredClause
        | D::EndMarker
        | D::Constraint
        | D::TerminationMeasure
        | D::Instantiation
        | D::Field
        | D::Variant => return None,
    })
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
    let candidate_ty = hir::Ty::named(ret_str);
    context::compute_type_match(expected, &candidate_ty)
}
