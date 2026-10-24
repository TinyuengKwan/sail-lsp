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
use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;
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
        acc.add(
            CompletionItem::new(CompletionItemKind::Keyword, "_")
                .detail("wildcard pattern")
                .insert_text("_")
                // The wildcard is always applicable and almost never what
                // the cursor wants first.
                .sort_text("zzz_wildcard")
                .filter_text("_"),
        );
    }

    // 2. Boolean literals
    for lit in &["true", "false"] {
        if prefix_lower.is_empty() || lit.starts_with(&prefix_lower) {
            acc.add(
                CompletionItem::new(CompletionItemKind::Keyword, *lit)
                    .detail("boolean pattern")
                    .insert_text(*lit)
                    .filter_text(*lit),
            );
        }
    }

    // 3. Enum members and union constructors from workspace
    let mut seen = std::collections::HashSet::new();
    for (_uri, file) in all_files {
        if let Some(tree) = file.item_tree() {
            // The members of every `enum` and `union`, which the item tree
            // carries as the definition's fields.
            let members = tree
                .top_level_items()
                .iter()
                .filter(|id| {
                    matches!(
                        id.def_kind(tree),
                        hir::DefKind::Enum | hir::DefKind::Union | hir::DefKind::Newtype
                    ) || id.scattered_type_kind(tree).is_some()
                })
                .flat_map(|id| {
                    id.fields(tree)
                        .iter()
                        .filter(|f| ctx.can_see_member(*file, f))
                        .map(|f| f.name.to_string())
                });
            for name in members {
                if !prefix_lower.is_empty() && !name.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                if !seen.insert(name.clone()) {
                    continue;
                }
                acc.add(
                    crate::render::pattern::render_variant_pat(
                        &name,
                        Some("enum member"),
                        Some(&name),
                    )
                    .filter_text(name),
                );
            }

            // A union's constructors take arguments, so they are offered
            // with parentheses. Same source as the members above — a
            // `union`'s fields *are* its constructors.
            let ctors: Vec<String> = tree
                .top_level_items()
                .iter()
                .filter(|id| {
                    matches!(id.def_kind(tree), hir::DefKind::Union | hir::DefKind::Newtype)
                        || id.scattered_type_kind(tree) == Some(hir::TypeDefShape::Union)
                })
                .flat_map(|id| {
                    id.fields(tree)
                        .iter()
                        .filter(|f| ctx.can_see_member(*file, f))
                        .map(|f| f.name.to_string())
                })
                .collect();
            for ctor in &ctors {
                if !prefix_lower.is_empty() && !ctor.to_ascii_lowercase().starts_with(&prefix_lower)
                {
                    continue;
                }
                if !seen.insert(ctor.clone()) {
                    continue;
                }
                // Offered with parens, for the constructors that take arguments.
                acc.add(
                    crate::render::pattern::render_variant_pat(
                        ctor,
                        Some("union constructor"),
                        Some(&format!("{ctor}($0)")),
                    )
                    .filter_text(ctor),
                );
            }
        }
    }
}
