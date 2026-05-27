//! Pattern completion — enum variants, wildcards, booleans.
//!
//! Activated when cursor is in `CompletionPosition::Pattern`
//! (inside match arms, let bindings, function clause parameters).
//!
//! - Enum variant constructors
//! - Wildcard `_`
//! - Boolean literals `true` / `false`
//! - Tuple patterns

use super::Completions;
use ide_db::ide_types::{CompletionItem, CompletionItemKind};
use ide_db::FileDb;
use url::Url;

use crate::context::{CompletionContext, CompletionPosition};

/// Complete patterns (enum variants, wildcards, booleans).
pub(crate) fn complete_pattern(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
) {
    // Only active in Pattern position
    if ctx.position != CompletionPosition::Pattern {
        return;
    }

    let prefix_lower = ctx.prefix_lower();

    // 1. Wildcard `_` — always available in patterns
    if prefix_lower.is_empty() || "_".starts_with(&prefix_lower) {
        acc.add(CompletionItem {
            label: "_".to_string(),
            kind: CompletionItemKind::Keyword,
            detail: Some("wildcard pattern".to_string()),
            documentation: None,
            insert_text: Some("_".to_string()),
            text_edit: None,
            sort_text: Some("zzz_wildcard".to_string()), // sort last
            filter_text: Some("_".to_string()),
            deprecated: false,
            relevance: Default::default(),
        });
    }

    // 2. Boolean literals
    for lit in &["true", "false"] {
        if prefix_lower.is_empty() || lit.starts_with(&prefix_lower) {
            acc.add(CompletionItem {
                label: lit.to_string(),
                kind: CompletionItemKind::Keyword,
                detail: Some("boolean pattern".to_string()),
                documentation: None,
                insert_text: Some(lit.to_string()),
                text_edit: None,
                sort_text: None,
                filter_text: Some(lit.to_string()),
                deprecated: false,
                relevance: Default::default(),
            });
        }
    }

    // 3. Enum members and union constructors from workspace
    let mut seen = std::collections::HashSet::new();
    for (_uri, file) in all_files {
        if let Some(parsed) = file.parsed() {
            for decl in &parsed.decls {
                if decl.scope != syntax::parser_lower::Scope::TopLevel {
                    continue;
                }
                // Only include constructors (enum members, union variants)
                let is_constructor =
                    matches!(decl.kind, syntax::parser_lower::DeclKind::EnumMember);
                if !is_constructor {
                    continue;
                }
                if !prefix_lower.is_empty()
                    && !decl.name.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                if !seen.insert(decl.name.clone()) {
                    continue;
                }
                acc.add(CompletionItem {
                    label: decl.name.clone(),
                    kind: CompletionItemKind::EnumMember,
                    detail: Some("enum member".to_string()),
                    documentation: None,
                    insert_text: Some(decl.name.clone()),
                    text_edit: None,
                    sort_text: None,
                    filter_text: Some(decl.name.clone()),
                    deprecated: false,
                    relevance: Default::default(),
                });
            }

            // Also add union constructor names from ParsedFile
            for ctor in &parsed.union_constructor_names {
                if !prefix_lower.is_empty() && !ctor.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                if !seen.insert(ctor.clone()) {
                    continue;
                }
                acc.add(CompletionItem {
                    label: ctor.clone(),
                    kind: CompletionItemKind::EnumMember,
                    detail: Some("union constructor".to_string()),
                    documentation: None,
                    // Offer with parens for constructors that take arguments
                    insert_text: Some(format!("{}($0)", ctor)),
                    text_edit: None,
                    sort_text: None,
                    filter_text: Some(ctor.clone()),
                    deprecated: false,
                    relevance: Default::default(),
                });
            }
        }
    }
}
