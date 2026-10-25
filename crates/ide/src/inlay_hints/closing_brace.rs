//! Closing brace label hints.

use super::*;

pub(super) fn collect_closing_brace_hints(
    file: &dyn FileDb,
    begin: usize,
    end: usize,
    min_lines: usize,
    hints: &mut Vec<InlayHint>,
) {
    let Some(tree) = file.item_tree() else {
        return;
    };

    for &id in tree.top_level_items() {
        let span = id.span(tree);
        let start_lc = file.position_at(span.start);
        let end_lc = file.position_at(span.end);
        if ((end_lc.line - start_lc.line) as usize) < min_lines {
            continue;
        }
        if span.end < begin || span.end > end {
            continue;
        }
        hints.push(InlayHint {
            offset: span.end,
            label: format!(" // {}", id.name(tree)),
            kind: InlayHintKind::Other,
            tooltip: Some(format!("End of {}", id.name(tree))),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
}
