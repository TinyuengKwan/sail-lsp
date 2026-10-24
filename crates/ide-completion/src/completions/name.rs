//! Completion at a name *definition* site — `function fo|`, `mapping fo|`.
//!
//! A new name is not a reference, so nothing that already exists belongs
//! here — with one exception, and it is the same exception RA makes when
//! it completes a trait method inside an `impl`: a declaration that is
//! still waiting for its definition. In Sail that is a `val` spec (or a
//! `mapping` spec) with no `function` of that name anywhere.
//!
//! `analyze` used to answer `NameRef` at every position, so this site was
//! offered every symbol in the workspace instead.

use ide_db::FileDb;
use url::Url;

use crate::completions::Completions;
use crate::context::{self, NameKind};
use crate::render::render_pending_definition;

pub(crate) fn complete_name(
    acc: &mut Completions,
    ctx: &context::CompletionContext<'_>,
    all_files: &[(&Url, &dyn FileDb)],
    name_ctx: &context::NameContext,
) {
    // Only a callable definition can be answered by a pending signature.
    // A new type, binding or register name has no such source.
    if name_ctx.kind != NameKind::Function {
        return;
    }

    let mut defined: Vec<String> = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    for (_uri, file) in all_files {
        let Some(tree) = file.item_tree() else {
            continue;
        };
        for &id in tree.top_level_items() {
            let name = id.name(tree).to_string();
            if !ctx.can_see_item(*file, tree, id) {
                continue;
            }
            match id.def_kind(tree) {
                hir::DefKind::Function | hir::DefKind::Mapping => defined.push(name),
                hir::DefKind::ValSpec | hir::DefKind::MappingSpec => declared.push(name),
                _ => {}
            }
        }
    }

    let prefix_lower = ctx.prefix_lower();
    declared.sort_unstable();
    declared.dedup();
    for name in declared {
        if defined.contains(&name) || !name.to_ascii_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        acc.add(render_pending_definition(&name));
    }
}
