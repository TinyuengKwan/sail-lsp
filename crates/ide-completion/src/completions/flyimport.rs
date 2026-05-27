//! Auto-import completion provider.
//!
//! Suggests items from other files and auto-inserts `$include` directives.

use super::Completions;
use ide_db::ide_types::{CompletionItem, CompletionItemKind, CompletionRelevance};
use ide_db::FileDb;
use url::Url;

use crate::context::{CompletionContext, CompletionPosition};

/// Suggest symbols from other files, with auto-$include.
pub(crate) fn import_on_the_fly(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
) {
    // Only suggest imports for identifier-like positions.
    if ctx.position == CompletionPosition::Pattern {
        return;
    }
    if ctx.prefix.is_empty() || ctx.prefix.len() < 2 {
        return; // Too short to search
    }

    let prefix_lower = ctx.prefix_lower();

    // Collect names already visible in the current file.
    let visible_names: std::collections::HashSet<String> = {
        let mut names = std::collections::HashSet::new();
        if let Some(parsed) = ctx.file.parsed() {
            for entry in &parsed.callable_heads {
                names.insert(entry.name.clone());
            }
        }
        if let Some(item_tree) = ctx.file.item_tree() {
            for &id in item_tree.top_level_items() {
                names.insert(id.name(&item_tree).as_str().to_string());
            }
        }
        names
    };

    // Search all other files for matching symbols.
    for (uri, file) in all_files {
        if *uri == current_uri {
            continue;
        }
        let Some(item_tree) = file.item_tree() else {
            continue;
        };
        for &id in item_tree.top_level_items() {
            let name = id.name(&item_tree).as_str();
            let name_lower = name.to_ascii_lowercase();

            if !name_lower.starts_with(&prefix_lower) {
                continue;
            }
            if visible_names.contains(name) {
                continue;
            }

            let kind = match id.item_kind(&item_tree) {
                hir_def::ItemKind::Function | hir_def::ItemKind::Mapping => {
                    CompletionItemKind::Function
                }
                hir_def::ItemKind::Struct
                | hir_def::ItemKind::Union
                | hir_def::ItemKind::Enum
                | hir_def::ItemKind::Bitfield
                | hir_def::ItemKind::Newtype
                | hir_def::ItemKind::TypeAlias => CompletionItemKind::Struct,
                _ => CompletionItemKind::Variable,
            };

            acc.add(CompletionItem {
                label: name.to_string(),
                kind,
                detail: Some(format!("(auto-include from {})", uri.path())),
                documentation: None,
                insert_text: None,
                text_edit: None,
                sort_text: None,
                filter_text: None,
                deprecated: false,
                relevance: CompletionRelevance {
                    is_local: false,
                    requires_import: true,
                    ..Default::default()
                },
            });
        }
    }
}

// Tests for flyimport are integration-level in sail-lsp/src/tests.rs,
// as they require a full FileDb implementation with workspace context.
