//! `simplify_boolean` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn simplify_boolean(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = simplify_boolean_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("simplify_boolean", AssistKind::RefactorRewrite),
        "Simplify boolean",
        ctx.range,
        te,
    );
    Some(())
}

/// Code action: simplify boolean expressions.
/// `x == true` → `x`, `x == false` → `~(x)`, `x != true` → `~(x)`, `x != false` → `x`
pub fn simplify_boolean_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text().get(base_db::range_start(range)..base_db::range_end(range))?.trim();

    let simplified = if let Some(lhs) = text.strip_suffix("== true") {
        lhs.trim().to_string()
    } else if let Some(lhs) = text.strip_suffix("== false") {
        format!("~({})", lhs.trim())
    } else if let Some(lhs) = text.strip_suffix("!= true") {
        format!("~({})", lhs.trim())
    } else if let Some(lhs) = text.strip_suffix("!= false") {
        lhs.trim().to_string()
    } else if text == "not(true)" || text == "~(true)" {
        "false".to_string()
    } else if text == "not(false)" || text == "~(false)" || text == "true & true" {
        "true".to_string()
    } else if text.starts_with("true & ")
        || text.starts_with("false & ")
        || text.ends_with(" & false")
    {
        if text.contains("false") {
            "false".to_string()
        } else {
            return None;
        }
    } else if text.starts_with("false | ") || text.ends_with(" | false") {
        let non_false = text.replace("false | ", "").replace(" | false", "");
        non_false.trim().to_string()
    } else {
        return None;
    };

    if simplified.trim() == text {
        return None;
    }
    Some(vec![IdeTextEdit { range, new_text: simplified }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            simplify_boolean,
            "val f : bool -> bool
function f(b) = $0b == true$0
",
            expect![[r#"
    val f : bool -> bool
    function f(b) = b
"#]],
        );
    }
}
