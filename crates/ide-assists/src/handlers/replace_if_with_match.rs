//! `replace_if_with_match` assist.
//! Converts an if-then-else expression into a match expression.
//! This is the reverse of `convert_match_to_if`.
//!
//! ```sail
//! if x == Some(y) then expr1 else expr2
//! ```
//! →
//! ```sail
//! match x {
//!   Some(y) => expr1,
//!   _ => expr2,
//! }
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use parser::SyntaxKind as SK;
use syntax::SyntaxNode;

/// Sail adaptation: converts `if cond then body else other` to match.
///
/// The structure came out of the token stream before — `if` within ±50
/// bytes of the cursor, then a scan for `then`, then a depth count for
/// the matching `else` — and the condition was taken apart by looking for
/// the *substring* `" == "`, so `if s == "a == b" then` split at the wrong
/// one and `if x==y then` was not recognised at all. An `IF_EXPR` has the
/// three parts as children and a `BIN_EXPR` has its operator as a token.
pub(crate) fn replace_if_with_match(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();
    let root = ctx.source_file()?;

    let kw = root.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| {
        t.kind() == SK::KW_IF
            && usize::from(t.text_range().start()) <= offset
            && offset < usize::from(t.text_range().end())
    })?;
    let if_expr = kw.parent().filter(|n| n.kind() == SK::IF_EXPR)?;

    let mut parts = if_expr.children();
    let cond = parts.next()?;
    let then_body = parts.next()?;
    let else_body = parts.next().map(|n| n.to_string()).unwrap_or_else(|| "()".to_owned());
    let then_body = then_body.to_string();

    // `(scrutinee, first arm's pattern and body, second arm's body)`.
    //
    // The negated forms used to emit `true => then, true => else` — the
    // same pattern twice, so the second arm was unreachable and the
    // `else` branch was lost.
    let (scrutinee, first_pat, first_body, second_body) = match analyze_condition(&cond) {
        Condition::Bool => (cond.to_string(), "true".to_owned(), then_body, else_body),
        Condition::Not(inner) => (inner, "false".to_owned(), then_body, else_body),
        Condition::Eq(lhs, rhs) => (lhs, rhs, then_body, else_body),
        // `if x != p then a else b` matches `p` in the `else` case.
        Condition::Ne(lhs, rhs) => (lhs, rhs, else_body, then_body),
    };

    let indent = detect_indent(text, usize::from(if_expr.text_range().start()));
    let match_text = format!(
        "match {scrutinee} {{\n\
         {indent}  {first_pat} => {first_body},\n\
         {indent}  _ => {second_body},\n\
         {indent}}}",
    );

    let target = base_db::text_range(
        usize::from(kw.text_range().start()),
        usize::from(kw.text_range().end()),
    );
    acc.add(
        AssistId("replace_if_with_match", AssistKind::RefactorRewrite),
        "Convert if to match",
        target,
        |builder| {
            let mut editor = builder.make_editor(&if_expr);
            editor.replace(if_expr.clone(), syntax::ast::make::expr_from_text(&match_text));
            builder.add_file_edits(editor);
        },
    );
    Some(())
}

/// What a condition says about the value being matched.
enum Condition {
    /// `if x` — match `x` against `true`.
    Bool,
    /// `not(x)` — match `x` against `false`.
    Not(String),
    /// `x == p` — match `x` against `p`.
    Eq(String, String),
    /// `x != p` — match `x` against `p`, with the branches swapped.
    Ne(String, String),
}

fn analyze_condition(cond: &SyntaxNode) -> Condition {
    if cond.kind() == SK::BIN_EXPR {
        let op = cond
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| matches!(t.text(), "==" | "!="));
        if let Some(op) = op {
            let mut sides = cond.children();
            if let (Some(lhs), Some(rhs)) = (sides.next(), sides.next()) {
                let (lhs, rhs) = (lhs.to_string(), rhs.to_string());
                return if op.text() == "==" {
                    Condition::Eq(lhs, rhs)
                } else {
                    Condition::Ne(lhs, rhs)
                };
            }
        }
    }
    if cond.kind() == SK::CALL_EXPR {
        let mut children = cond.children();
        if let Some(callee) = children.next() {
            if callee.to_string() == "not" {
                // The argument, looking through the `ARG_LIST`/parens the
                // parser wraps a call's arguments in.
                let arg = children
                    .next()
                    .map(|a| a.to_string())
                    .map(|a| a.trim_start_matches('(').trim_end_matches(')').trim().to_owned());
                if let Some(arg) = arg {
                    return Condition::Not(arg);
                }
            }
        }
    }
    Condition::Bool
}

/// Detect indentation at a given offset.
fn detect_indent(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line = &text[line_start..offset];
    line.chars().take_while(|c| c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            replace_if_with_match,
            "val f : int -> int\nfunction f(x) = $0if x > 0 then 1 else 2\n",
            expect![[r#"
            val f : int -> int
            function f(x) = match x > 0 {
              true => 1,
              _ => 2,
            }
        "#]],
        );
    }

    /// `not(x)` used to produce `true => then, true => else` — the same
    /// pattern twice, with the `else` branch unreachable.
    #[test]
    fn a_negated_condition_matches_false() {
        check_assist(
            replace_if_with_match,
            "val f : bool -> int\nfunction f(b) = $0if not(b) then 1 else 2\n",
            expect![[r#"
            val f : bool -> int
            function f(b) = match b {
              false => 1,
              _ => 2,
            }
        "#]],
        );
    }

    /// The substring search for `" == "` needed the spaces, and found the
    /// first one anywhere in the condition's text.
    #[test]
    fn an_equality_without_spaces_is_still_an_equality() {
        check_assist(
            replace_if_with_match,
            "val f : int -> int\nfunction f(x) = $0if x==3 then 1 else 2\n",
            expect![[r#"
            val f : int -> int
            function f(x) = match x {
              3 => 1,
              _ => 2,
            }
        "#]],
        );
    }
}
