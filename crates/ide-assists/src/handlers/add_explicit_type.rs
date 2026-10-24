//! `add_explicit_type` assist.
//! Adds an explicit type annotation to a `let` or `var` binding
//! when the type can be inferred.
//!
//! ```sail
//! let x = 42
//! ```
//! →
//! ```sail
//! let x : atom(42) = 42
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};

/// Triggers when cursor is on a `let`/`var` binding name that has no
/// type annotation. Uses `FileDb::binding_type_text()` to get the
/// inferred type from the salsa query.
pub(crate) fn add_explicit_type(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    use parser::SyntaxKind as SK;

    let root = ctx.source_file()?;
    let offset = ctx.offset();

    // The binding the cursor is on.
    //
    // This used to scan the token stream backwards up to 200 bytes for
    // the nearest `let` or `var`, then forwards up to 50 for an
    // identifier, then forwards through the *rest of the file* for a `:`
    // before an `=` to decide whether a type was already written. A
    // `LET_EXPR`'s pattern is either an `IDENT_PAT` or a `TYPED_PAT`, and
    // that is the whole question.
    let let_expr = root
        .descendants()
        .filter(|n| {
            matches!(n.kind(), SK::LET_EXPR | SK::VAR_EXPR)
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;
    let pat = let_expr.children().next()?;
    // A `TYPED_PAT` already says what it is.
    if pat.kind() != SK::IDENT_PAT {
        return None;
    }
    let name_range = pat.text_range();
    let name_span = parser::Span::new(name_range.start().into(), name_range.end().into());
    if offset > name_span.end + 1 {
        return None;
    }

    // The inferred type, from the salsa query.
    let ty_text = ctx.file.binding_type_text(name_span)?;
    if ty_text == "unknown" || ty_text == "error" || ty_text == "unit" {
        return None;
    }

    let insert_offset = name_span.end;
    let insert_text = format!(" : {ty_text}");

    let target = base_db::text_range(name_span.start, name_span.end);
    acc.add(
        AssistId("add_explicit_type", AssistKind::RefactorRewrite),
        format!("Insert explicit type `{ty_text}`"),
        target,
        |builder| {
            builder.insert(insert_offset, insert_text.clone());
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            add_explicit_type,
            "function f() = { let x$0 = 42; x }\n",
            expect![[r#"
                function f() = { let x : atom(42) = 42; x }
            "#]],
        );
    }

    /// A binding that already says its type.
    #[test]
    fn declines_when_the_type_is_written() {
        check_assist_not_applicable(
            add_explicit_type,
            "function f() = { let y$0 : int = 42; y }\n",
        );
    }

    /// The backwards scan matched the word `let` inside a string.
    #[test]
    fn a_let_in_a_string_is_not_a_binding() {
        check_assist_not_applicable(
            add_explicit_type,
            "function f() = { print(\"let z = 1\"); z$0 }\n",
        );
    }
}
