//! Caller count hints at function definition sites.
//!
//! Sail-specific — no direct RA counterpart.

use super::*;

pub(super) fn collect_callgraph_caller_hints<'a>(
    all_files: &[(&'a Url, &'a dyn FileDb)],
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<IdeDbInlayHint>,
) {
    let workspace_files: Vec<&dyn FileDb> = all_files.iter().map(|(_, f)| *f).collect();
    let workspace_callgraph =
        hir_def::callgraph::cached_workspace_callgraph(workspace_files.iter().copied());
    // Use ParsedFile callable_heads instead of core_ast.defs
    let Some(parsed) = current_file.parsed() else {
        return;
    };
    for head in &parsed.callable_heads {
        let name_span = head.name_span;
        if !span_starts_in_range(name_span, begin, end) {
            continue;
        }
        let name = head.name.as_str();
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
        hints.push(IdeDbInlayHint {
            offset: name_span.end,
            label,
            kind: IdeDbInlayHintKind::Other,
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
