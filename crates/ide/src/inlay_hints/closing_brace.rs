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
        if !span_ends_in_range(span, begin, end) {
            continue;
        }
        let start_lc = file.position_at(span.start);
        let end_lc = file.position_at(span.end);
        if ((end_lc.line - start_lc.line) as usize) < min_lines {
            continue;
        }
        hints.push(InlayHint {
            label_parts: Vec::new(),
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
