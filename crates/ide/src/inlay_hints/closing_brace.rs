//! Closing brace label hints.

use super::*;

pub(super) fn collect_closing_brace_hints(
    file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<IdeDbInlayHint>,
) {
    let Some(parsed) = file.parsed() else {
        return;
    };

    for decl in &parsed.decls {
        if decl.scope != syntax::parser_lower::Scope::TopLevel {
            continue;
        }
        let start_lc = file.position_at(decl.span.start);
        let end_lc = file.position_at(decl.span.end);
        if end_lc.line - start_lc.line <= 5 {
            continue;
        }
        if decl.span.end < begin || decl.span.end > end {
            continue;
        }
        hints.push(IdeDbInlayHint {
            offset: decl.span.end,
            label: format!(" // {}", decl.name),
            kind: IdeDbInlayHintKind::Other,
            tooltip: Some(format!("End of {}", decl.name)),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
}
