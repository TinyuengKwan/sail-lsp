//! `inline_call` assist.
//! Inlines a function call by replacing it with the function body,
//! substituting formal parameters with actual arguments.
//!
//! ```sail
//! function double(x : int) -> int = x + x
//!
//! let y = double(3)
//! ```
//! →
//! ```sail
//! function double(x : int) -> int = x + x
//!
//! let y = 3 + 3
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;
use syntax::{SyntaxKind as SK, SyntaxNode};

pub(crate) fn inline_call(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = inline_call_edits(ctx.file, ctx.offset())?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_call", AssistKind::RefactorInline),
        "Inline function call",
        ctx.range,
        te,
    );
    Some(())
}

/// Replace the call whose callee is at `offset` with the callee's body,
/// each parameter replaced by its argument.
///
/// From the tree: the body is the expression after the definition's `=`,
/// and only a parameter used as a value (`IDENT_EXPR`) is replaced. This
/// worked on text and was wrong in four ways — the body began at the first
/// `=` (inside `>=` in a return type), every same-spelled word was replaced
/// (a field, a shadowing `let`), and neither the arguments nor the result
/// were parenthesised, so `2 * double(a + b)` became `2 * a + b + a + b`.
///
/// Declines where substitution cannot be done by spelling: a parameter
/// that is a pattern rather than a name, an argument count that differs, a
/// body that rebinds a parameter, or one that binds a name an argument uses.
pub fn inline_call_edits(file: &dyn FileDb, offset: usize) -> Option<Vec<IdeTextEdit>> {
    // A file without a stored tree is parsed, as `AssistContext` does.
    let root = file.syntax().unwrap_or_else(|| syntax::parse_text(file.text()).0);
    let token = root.token_at_offset((offset as u32).into()).find(|t| t.kind() == SK::IDENT)?;
    let callee = token.parent().filter(|n| n.kind() == SK::IDENT_EXPR)?;
    let call = callee.parent().filter(|n| n.kind() == SK::CALL_EXPR)?;
    if call.first_child()? != callee {
        return None;
    }
    let args: Vec<SyntaxNode> =
        call.children().find(|n| n.kind() == SK::ARG_LIST)?.children().collect();
    let name = token.text();

    let def = root.children().find(|n| {
        n.kind() == SK::CALLABLE_DEF
            && n.children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .is_some_and(|t| t.text() == name)
    })?;
    let params = formal_params(&def)?;
    if params.len() != args.len() {
        return None;
    }
    let after_eq = def
        .children_with_tokens()
        .skip_while(|e| e.kind() != SK::EQ)
        .skip(1)
        .find_map(|e| e.into_node())?;

    // Names the body binds, and every identifier the arguments use.
    let binders: Vec<String> = after_eq
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SK::IDENT && t.parent().is_some_and(|p| p.kind() == SK::IDENT_PAT))
        .map(|t| t.text().to_owned())
        .collect();
    if binders.iter().any(|b| params.contains(b)) {
        return None;
    }
    let used_by_args = args.iter().flat_map(|a| {
        a.descendants_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == SK::IDENT)
    });
    if used_by_args.into_iter().any(|t| binders.iter().any(|b| b == t.text())) {
        return None;
    }

    // Rebuild the body's text, a parameter mention replaced by its argument.
    let body_start = usize::from(after_eq.text_range().start());
    let mut body = after_eq.text().to_string();
    let mut mentions: Vec<(usize, usize, usize)> = after_eq
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SK::IDENT && t.parent().is_some_and(|p| p.kind() == SK::IDENT_EXPR))
        .filter_map(|t| {
            let index = params.iter().position(|p| p == t.text())?;
            let r = t.text_range();
            Some((usize::from(r.start()) - body_start, usize::from(r.end()) - body_start, index))
        })
        .collect();
    mentions.sort_by_key(|m| std::cmp::Reverse(m.0));
    for (start, end, index) in mentions {
        body.replace_range(start..end, &parenthesized(&args[index]));
    }
    let new_text = if is_atomic(&after_eq) { body } else { format!("({body})") };
    let range = call.text_range();
    Some(vec![IdeTextEdit {
        range: base_db::text_range(usize::from(range.start()), usize::from(range.end())),
        new_text,
    }])
}

/// The parameter names of a definition, or `None` if any is a pattern
/// rather than a name.
fn formal_params(def: &SyntaxNode) -> Option<Vec<String>> {
    let list = def.children().find(|n| n.kind() == SK::PARAM_LIST);
    let Some(list) = list else {
        // `f()` lowers to UNIT: no parameters.
        return def.children_with_tokens().any(|e| e.kind() == SK::UNIT).then(Vec::new);
    };
    let mut names = Vec::new();
    for element in list.children_with_tokens() {
        match element.kind() {
            SK::IDENT => names.push(element.into_token()?.text().to_owned()),
            SK::L_PAREN | SK::R_PAREN | SK::COMMA | SK::COLON | SK::WHITESPACE => {}
            kind if kind.is_type_node() => {}
            _ => return None,
        }
    }
    Some(names)
}

fn is_atomic(expr: &SyntaxNode) -> bool {
    matches!(
        expr.kind(),
        SK::IDENT_EXPR
            | SK::LITERAL_EXPR
            | SK::TUPLE_EXPR
            | SK::CALL_EXPR
            | SK::BLOCK_EXPR
            | SK::FIELD_ACCESS_EXPR
            | SK::INDEX_EXPR
            | SK::SUBRANGE_EXPR
    )
}

fn parenthesized(expr: &SyntaxNode) -> String {
    if is_atomic(expr) {
        expr.text().to_string()
    } else {
        format!("({})", expr.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    /// Arguments and the result keep their grouping.
    #[test]
    fn grouping_survives() {
        check_assist(
            inline_call,
            "function dbl(x : int) -> int = x + x\nfunction g(a : int) -> int = 2 * $0dbl(a + 1)\n",
            expect![[r#"
            function dbl(x : int) -> int = x + x
            function g(a : int) -> int = 2 * ((a + 1) + (a + 1))
        "#]],
        );
    }

    /// The body starts after the definition's `=`, not at a `>=`, and a
    /// field spelled like a parameter is not one.
    #[test]
    fn the_body_and_its_mentions_come_from_the_tree() {
        check_assist(
            inline_call,
            "struct S = { x : int }\n\
             function get(x : S) -> {'n, 'n >= 0. int('n)} = x.x\n\
             function g(s : S) -> int = $0get(s)\n",
            expect![[r#"
            struct S = { x : int }
            function get(x : S) -> {'n, 'n >= 0. int('n)} = x.x
            function g(s : S) -> int = s.x
        "#]],
        );
    }

    /// A body that would capture an argument's name is not inlined.
    #[test]
    fn a_capturing_body_is_not_inlined() {
        crate::tests::check_assist_not_applicable(
            inline_call,
            "function f(x : int) -> int = { let y : int = 1; x + y }\n\
             function g(y : int) -> int = $0f(y)\n",
        );
    }

    #[test]
    fn applies() {
        check_assist(
            inline_call,
            "val g : int -> int\nfunction g(x) = x\nval f : int -> int\nfunction f(x) = $0g(x)\n",
            expect![[r#"
            val g : int -> int
            function g(x) = x
            val f : int -> int
            function f(x) = x
        "#]],
        );
    }
}
