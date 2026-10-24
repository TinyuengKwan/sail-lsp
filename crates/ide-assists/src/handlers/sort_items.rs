//! `sort_items` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn sort_items(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = sort_items_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("sort_items", AssistKind::RefactorRewrite),
        "Sort items",
        ctx.range,
        te,
    );
    Some(())
}

pub fn sort_items_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    use hir::DefKind;
    let offset = base_db::range_start(range);
    let item_tree = file.item_tree()?;
    let text = file.text();

    // Find enum/struct/union at cursor
    for &id in item_tree.top_level_items() {
        let span = id.span(item_tree);
        if offset < span.start || offset > span.end {
            continue;
        }
        if !matches!(id.def_kind(item_tree), DefKind::Enum | DefKind::Struct | DefKind::Union) {
            continue;
        }

        // Extract the body between { and } from source text
        let def_text = text.get(span.start..span.end)?;
        let brace_start = def_text.find('{')?;
        let brace_end = def_text.rfind('}')?;
        if brace_start >= brace_end {
            return None;
        }

        let inner = &def_text[brace_start + 1..brace_end];

        // Detect the indentation used in the original body.
        let body_indent = inner
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| {
                let trimmed = line.trim_start();
                &line[..line.len() - trimmed.len()]
            })
            .unwrap_or("  ");

        // Parse items with attached comments.
        // Comments preceding an item are kept together with it during
        // sorting so they maintain their association.
        //
        // Example input:
        //   // Machine external
        //   MEI : 11,
        //   // Machine timer
        //   MTI : 7,
        //
        // After sorting: comments stay with their field.
        struct ItemWithComment<'a> {
            /// Lines of comment text preceding this item (with indent).
            comment_lines: Vec<&'a str>,
            /// The item text (trimmed, without trailing comma).
            item_text: &'a str,
            /// Sort key: first identifier in the item.
            sort_key: &'a str,
        }
        let mut items_with_comments: Vec<ItemWithComment<'_>> = Vec::new();
        let mut pending_comment_lines: Vec<&str> = Vec::new();

        // First strip comments from inner, then split by comma
        // Strategy: walk lines, accumulate comment lines, attach to next item
        for part in inner.split(',') {
            let lines: Vec<&str> = part.lines().collect();
            let mut item_line = None;
            for line in &lines {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with("//") {
                    pending_comment_lines.push(line);
                } else {
                    item_line = Some(trimmed);
                }
            }
            if let Some(item_text) = item_line {
                if item_text.is_empty() {
                    continue;
                }
                let sort_key = item_text
                    .split_whitespace()
                    .next()
                    .and_then(|w| w.split(':').next())
                    .unwrap_or(item_text);
                items_with_comments.push(ItemWithComment {
                    comment_lines: std::mem::take(&mut pending_comment_lines),
                    item_text,
                    sort_key,
                });
            }
        }

        if items_with_comments.len() < 2 {
            return None;
        }

        let already_sorted = items_with_comments.windows(2).all(|w| w[0].sort_key <= w[1].sort_key);
        if already_sorted {
            return None;
        }

        items_with_comments.sort_by_key(|item| item.sort_key);

        // Rebuild with comments attached before each item.
        let mut sorted_lines = String::from("\n");
        for item in &items_with_comments {
            for comment in &item.comment_lines {
                sorted_lines.push_str(comment);
                sorted_lines.push('\n');
            }
            sorted_lines.push_str(body_indent);
            sorted_lines.push_str(item.item_text);
            sorted_lines.push_str(",\n");
        }

        let abs_start = span.start + brace_start + 1;
        let abs_end = span.start + brace_end;
        return Some(vec![IdeTextEdit {
            range: base_db::text_range(abs_start, abs_end),
            new_text: sorted_lines,
        }]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            sort_items,
            "enum C = { $0B, A }
",
            expect![[r#"
    enum C = {
     A,
     B,
    }
"#]],
        );
    }
}
