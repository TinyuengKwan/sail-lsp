//! Caller count hints at function definition sites.
//!
//! Sail-specific — no direct RA counterpart.

use super::*;

pub(super) fn collect_callgraph_caller_hints<'a>(
    all_files: &[(&'a Url, &'a dyn FileDb)],
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    let visible_heads = visible_callable_heads(current_file, begin, end);
    if visible_heads.is_empty() {
        return;
    }
    let workspace_callgraph =
        hir::callgraph::cached_workspace_callgraph(all_files.iter().map(|(_, file)| *file));
    for (name, name_span) in visible_heads {
        let name = name.as_str();
        if name.starts_with("Mk_")
            || name.starts_with("_get_")
            || name.starts_with("_update_")
            || name.starts_with("_set_")
            || name.starts_with("_mod_")
        {
            continue;
        }
        let count = workspace_callgraph.site_count_to(name);
        if count == 0 {
            continue;
        }
        let label =
            if count == 1 { "(1 caller)".to_string() } else { format!("({count} callers)") };
        hints.push(InlayHint {
            label_parts: Vec::new(),
            offset: name_span.end,
            label,
            kind: InlayHintKind::Other,
            tooltip: Some(format!(
                "{name} is called from {count} site{} across the workspace",
                if count == 1 { "" } else { "s" }
            )),
            padding_left: Some(true),
            padding_right: Some(false),
            data: None,
        });
    }
}
