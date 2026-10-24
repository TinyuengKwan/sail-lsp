//! `generate_doc_template` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn generate_doc_template(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = generate_doc_template_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("generate_doc_template", AssistKind::RefactorRewrite),
        "Generate doc template",
        ctx.range,
        te,
    );
    Some(())
}

pub fn generate_doc_template_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let parsed = file.parsed()?;

    // Find the callable head at cursor offset
    for head in &parsed.callable_heads {
        if offset < head.label_span.start || offset > head.label_span.end {
            continue;
        }
        let text = file.text();
        let before = &text[..head.label_span.start];
        if before.trim_end().ends_with("*/") || before.trim_end().ends_with("///") {
            return None;
        }

        let mut doc = String::from("/*!\n");
        doc.push_str(&format!(" * {}\n", head.name));
        doc.push_str(" *\n");
        for param in &head.params {
            if let Some(name) = &param.name {
                doc.push_str(&format!(" * @param {name}\n"));
            }
        }
        doc.push_str(" * @return\n");
        doc.push_str(" */\n");

        return Some(vec![IdeTextEdit {
            range: base_db::text_range(head.label_span.start, head.label_span.start),
            new_text: doc,
        }]);
    }

    // Non-callable definitions: use ItemTree
    if let Some(it) = file.item_tree() {
        for &id in it.top_level_items() {
            let span = id.span(it);
            if offset < span.start || offset > span.end {
                continue;
            }
            let text = file.text();
            let before = &text[..span.start];
            if before.trim_end().ends_with("*/") || before.trim_end().ends_with("///") {
                return None;
            }
            let doc = "/*!\n * TODO: document\n */\n".to_string();
            return Some(vec![IdeTextEdit {
                range: base_db::text_range(span.start, span.start),
                new_text: doc,
            }]);
        }
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
            generate_doc_template,
            "val f : int -> int\nfunction $0f(x) = x\n",
            expect![[r#"
            val f : int -> int
            /*!
             * f
             *
             * @param x
             * @return
             */
            function f(x) = x
        "#]],
        );
    }
}
