//! `toggle_doc_comment` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn toggle_doc_comment(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = toggle_doc_comment_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("toggle_doc_comment", AssistKind::RefactorRewrite),
        "Toggle doc comment",
        ctx.range,
        te,
    );
    Some(())
}

/// Toggle between // and /// (doc comment)
pub fn toggle_doc_comment_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let start_offset = base_db::range_start(range);
    let end_offset = base_db::range_end(range);
    let region = text.get(start_offset..end_offset)?;

    let lines: Vec<&str> = region.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let is_doc = lines.iter().all(|l| l.trim().starts_with("///"));
    let is_normal = lines.iter().all(|l| {
        let t = l.trim();
        t.starts_with("//") && !t.starts_with("///")
    });

    if !is_doc && !is_normal {
        return None;
    }

    let new_text = if is_doc {
        // /// → //
        lines
            .iter()
            .map(|l| {
                let trimmed = l.trim();
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                let content = trimmed.strip_prefix("///").unwrap_or(trimmed);
                format!("{indent}//{content}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // // → ///
        lines
            .iter()
            .map(|l| {
                let trimmed = l.trim();
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                let content = trimmed.strip_prefix("//").unwrap_or(trimmed);
                format!("{indent}///{content}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Some(vec![IdeTextEdit { range, new_text }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            toggle_doc_comment,
            "$0// a note$0
val f : int -> int
",
            expect![[r#"
    /// a note
    val f : int -> int
"#]],
        );
    }
}
