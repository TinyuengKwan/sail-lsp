//! `add_braces` assist — wrap single-line if/else bodies in `{ }`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

pub(crate) fn add_braces(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The innermost `if` the cursor is in.
    //
    // This used to find the word `then` with `text.find(" then ")` and
    // slice from there, which matches inside a string literal or a comment,
    // misses a `then` followed by a newline, and guessed where the body
    // ended by scanning for the next line break.
    let if_expr = root
        .descendants()
        .filter(|n| n.kind() == SK::IF_EXPR)
        .filter(|n| {
            usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    // `if cond then body [else body]` — the branches are the children after
    // the condition, in order.
    let branches: Vec<_> = if_expr
        .children()
        .filter(|c| !c.kind().is_trivia())
        .skip(1)
        .filter(|c| c.kind() != SK::BLOCK_EXPR)
        .collect();
    if branches.is_empty() {
        return None;
    }

    acc.add(
        AssistId("add_braces", AssistKind::RefactorRewrite),
        "Add braces",
        ctx.range,
        |builder| {
            let mut editor = builder.make_editor(&if_expr);
            for branch in &branches {
                let braced = syntax::ast::make::expr_from_text(&format!(
                    "{{ {} }}",
                    branch.text().to_string().trim()
                ));
                editor.replace(branch.clone(), braced);
            }
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    fn applied(src: &str, at: usize) -> Option<String> {
        let file = TestFile::new(src);
        let cfg = crate::AssistConfig::default();
        let mut acc = Assists::new(base_db::FileId::from_raw(0));
        add_braces(&mut acc, &AssistContext::new(&cfg, &file, base_db::text_range(at, at)));
        let assist = acc.finish().into_iter().next()?;
        let change = assist.source_change?;
        let edits: Vec<_> = change.source_file_edits.into_values().flatten().collect();
        let mut out = src.to_string();
        for edit in edits.iter().rev() {
            out.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        Some(out)
    }

    #[test]
    fn wraps_both_branches() {
        let src = "val f : int -> int\nfunction f(x) = if x > 0 then 1 else 2\n";
        let at = src.find("if x").expect("the if");
        assert_eq!(
            applied(src, at).as_deref(),
            Some("val f : int -> int\nfunction f(x) = if x > 0 then { 1 } else { 2 }\n")
        );
    }

    /// The old scan found `then` inside a string and sliced from there.
    #[test]
    fn a_string_holding_then_is_not_a_branch() {
        let src = "val f : unit -> string\nfunction f() = \" then \"\n";
        let at = src.find("\" then").expect("the literal");
        assert_eq!(applied(src, at), None, "there is no `if` here");
    }
}
