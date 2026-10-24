//! `line_to_block_comment` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn line_to_block_comment(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = line_to_block_comment_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("line_to_block_comment", AssistKind::RefactorRewrite),
        "Line to block comment",
        ctx.range,
        te,
    );
    Some(())
}

/// Convert line comments (//) to block comments (/* */) in the selection range
pub fn line_to_block_comment_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let start_offset = base_db::range_start(range);
    let end_offset = base_db::range_end(range);

    // Find consecutive // lines in the range
    let region = text.get(start_offset..end_offset)?;
    let lines: Vec<&str> = region.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let all_line_comments = lines.iter().all(|l| l.trim().starts_with("//"));
    if !all_line_comments {
        return None;
    }

    let mut block = String::from("/*\n");
    for line in &lines {
        let stripped = line.trim().strip_prefix("///").or_else(|| line.trim().strip_prefix("//"));
        if let Some(content) = stripped {
            block.push_str(&format!(" *{}\n", content));
        }
    }
    block.push_str(" */");

    Some(vec![IdeTextEdit { range, new_text: block }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            line_to_block_comment,
            "$0// one
// two$0
val f : int -> int
",
            expect![[r#"
    /*
     * one
     * two
     */
    val f : int -> int
"#]],
        );
    }
}
