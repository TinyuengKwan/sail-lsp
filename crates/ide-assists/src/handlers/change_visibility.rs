//! `change_visibility` assist — toggle private modifier.
//!
//! Sail uses `private` keyword instead of `pub`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;
use parser::SyntaxKind as SK;

pub(crate) fn change_visibility(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let offset = ctx.offset();
    let root = ctx.source_file()?;

    // The definition the cursor is on.
    //
    // This used to read the cursor's *line* and ask whether its first
    // word was `private`, `function`, `val`, `type`, `struct`, `enum` or
    // `union` — so it fired on a comment or a string that began that way,
    // it missed every definition written across two lines, and on a
    // `mapping`, `register`, `newtype` or `bitfield` it did nothing.
    let def = root.children().find(|n| {
        usize::from(n.text_range().start()) <= offset && offset <= usize::from(n.text_range().end())
    })?;
    let is_definition = def
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| format!("{:?}", t.kind()).starts_with("KW_"));
    if !is_definition {
        return None;
    }

    // `private` is its own node when it is there.
    match def.children().find(|n| n.kind() == SK::VISIBILITY) {
        Some(vis) => {
            // The keyword and the space after it.
            let start = usize::from(vis.text_range().start());
            let end = vis
                .next_sibling_or_token()
                .filter(|el| el.kind() == SK::WHITESPACE)
                .map_or(usize::from(vis.text_range().end()), |ws| {
                    usize::from(ws.text_range().end())
                });
            acc.add_with_edits(
                AssistId("change_visibility", AssistKind::RefactorRewrite),
                "Make public (remove `private`)",
                ctx.range,
                vec![TextEdit { range: base_db::text_range(start, end), new_text: String::new() }],
            );
        }
        None => {
            let start = usize::from(def.text_range().start());
            acc.add_with_edits(
                AssistId("change_visibility", AssistKind::RefactorRewrite),
                "Make private",
                ctx.range,
                vec![TextEdit {
                    range: base_db::text_range(start, start),
                    new_text: "private ".to_owned(),
                }],
            );
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn adds_private() {
        check_assist(
            change_visibility,
            "function f$0oo() -> unit = ()\n",
            expect![[r#"
            private function foo() -> unit = ()
        "#]],
        );
    }

    #[test]
    fn removes_private() {
        check_assist(
            change_visibility,
            "private function f$0oo() -> unit = ()\n",
            expect![[r#"
            function foo() -> unit = ()
        "#]],
        );
    }

    /// The line-first-word test only knew seven forms of definition.
    #[test]
    fn a_register_can_be_made_private_too() {
        check_assist(
            change_visibility,
            "register r$0 : bits(32)\n",
            expect![[r#"
            private register r : bits(32)
        "#]],
        );
    }

    /// It fired on any line beginning with one of its keywords.
    #[test]
    fn a_comment_that_begins_with_a_keyword_is_not_a_definition() {
        check_assist_not_applicable(change_visibility, "// funct$0ion foo() = ()\n");
    }
}
