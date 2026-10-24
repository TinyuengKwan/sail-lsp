//! Function parameter name completion.
//!
//! Scans other functions in the same file for parameters whose names
//! match the current prefix. Offers `name : type` suggestions.

use super::Completions;
use crate::context::CompletionContext;
use crate::CompletionItemKind;

/// Complete function parameter names based on parameters of other
/// functions in the same file.
pub(crate) fn complete_fn_param(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let prefix_lower = ctx.prefix.to_ascii_lowercase();

    // Every named parameter this file's callables declare, from the bodies
    // that lowered them: a `Pat::Typed` wrapping a `Pat::Bind` is `x : T`,
    // and its `ty_span` says where the type is written.
    //
    // The scrape found the parameter list by looking for the first `(` in the
    // declaration's text and splitting what followed on commas — which is a
    // parser, written a second time, that a nested `(` or a tuple parameter
    // gets wrong.
    let Some(bodies) = ctx.file.bodies() else {
        return;
    };
    let mut seen = std::collections::HashSet::new();

    for entry in bodies.entries() {
        for (_, pat) in entry.body.iter_pats() {
            let hir::Pat::Typed { inner, ty } = pat else { continue };
            let Some(hir::Pat::Bind(name)) = entry.body.pat(*inner) else { continue };
            let type_text = ty.trim();
            let name = name.as_str();
            if name.is_empty() || type_text.is_empty() {
                continue;
            }

            // Filter by prefix.
            if !prefix_lower.is_empty() && !name.to_ascii_lowercase().starts_with(&prefix_lower) {
                continue;
            }

            let label = format!("{name} : {type_text}");
            if !seen.insert(label.clone()) {
                continue; // Deduplicate.
            }
            let decl_name = entry.name.clone();

            acc.add(
                crate::item::CompletionItem::new(CompletionItemKind::Variable, label)
                    .detail(format!("param from {decl_name}"))
                    .insert_text(format!("{name} : {type_text}"))
                    .sort_text(format!("1{name}"))
                    .filter_text(name),
            );
        }
    }
}
