//! `remove_unused_param` assist.
//! Removes a function parameter that is never used in the body.
//! Also updates all call sites to remove the corresponding argument.
//!
//! ```sail
//! function foo(x : int, y : int) -> int = x
//! ```
//! → (cursor on `y`)
//! ```sail
//! function foo(x : int) -> int = x
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use parser::{Span, Token};

/// Detects when cursor is on a function parameter that is never referenced
/// in the function body. Offers to remove the parameter from the signature.
///
/// rust-analyzer's version rewrites the call sites too, through
/// `fn_def.usages(&ctx.sema)`. Ours edits the parameter list and the calls
/// in the same file, and declines when the function has a `val` — Sail
/// keeps the parameter *types* there, so removing one from the list alone
/// leaves a file that does not type-check. Producing a broken file is
/// worse than not firing; rewriting the `val`'s arrow type is what would
/// let it fire.
pub(crate) fn remove_unused_param(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();
    let bodies = ctx.file.bodies()?;

    // The parameter under the cursor, from the body that declares it.
    //
    // This used to scan the token stream for "an identifier between `(` and
    // `)`", which is also true of a parameter's *type*: on `function f(e : E)`
    // with the cursor on `E`, it offered to delete the type. It decided
    // "unused" by searching the body text for the name, so `bits` and `bool`
    // were offered too.
    let (param_name, param_span, body_span, fn_name, param_index) =
        bodies.entries().iter().find_map(|entry| {
            if offset < entry.def_span.start || offset > entry.def_span.end {
                return None;
            }
            entry.body.params.iter().enumerate().find_map(|(param_index, &param)| {
                // `x : T` wraps the binding; the name is the binding's.
                let bind = match entry.body.pat(param) {
                    Some(hir::Pat::Typed { inner, .. }) => *inner,
                    Some(hir::Pat::Bind(_)) => param,
                    _ => return None,
                };
                let hir::Pat::Bind(name) = entry.body.pat(bind)? else { return None };
                let span = entry.source_map.pat_syntax(bind)?;
                (span.start <= offset && offset <= span.end).then(|| {
                    (name.to_string(), span, entry.body_span, entry.name.clone(), param_index)
                })
            })
        })?;

    // Used anywhere in the body?
    let used = bodies.entries().iter().any(|entry| {
        entry.body_span == body_span
            && entry
                .body
                .iter_exprs()
                .any(|(_, e)| matches!(e, hir::Expr::Ident(n) if n.as_str() == param_name))
    });
    if used {
        return None;
    }

    // A `val` carries the parameter types, and this cannot rewrite one:
    // dropping the parameter from the list alone leaves a file that does
    // not type-check, which is worse than not firing.
    let has_val = ctx.file.item_tree().is_some_and(|tree| {
        tree.top_level_items().iter().any(|id| {
            matches!(id.def_kind(tree), hir::DefKind::ValSpec | hir::DefKind::MappingSpec)
                && id.name(tree).as_str() == fn_name
        })
    });
    if has_val {
        return None;
    }

    let tokens = ctx.file.tokens()?;
    let param_range = find_param_range_with_separator(text, tokens, &param_span)?;

    // The calls in this file lose the argument in that position too. Across
    // files they do not: that needs the workspace, which an assist here
    // does not have — rust-analyzer's version reaches them through
    // `fn_def.usages(&ctx.sema)`.
    let mut edits = vec![TextEdit { range: param_range, new_text: String::new() }];
    edits.extend(call_argument_edits(ctx, &fn_name, param_index));

    let target = base_db::text_range(param_span.start, param_span.end);
    acc.add_with_edits(
        AssistId("remove_unused_param", AssistKind::RefactorRewrite),
        format!("Remove unused parameter `{param_name}`"),
        target,
        edits,
    );
    Some(())
}

/// Find the full range of a parameter including its type annotation
/// and surrounding separator (comma or leading/trailing whitespace).
fn find_param_range_with_separator(
    _text: &str,
    tokens: &[(Token, Span)],
    param_span: &Span,
) -> Option<TextRange> {
    // Find the previous comma or `(` and next comma or `)`
    let mut prev_sep = None;
    let mut next_sep = None;

    for (tok, span) in tokens.iter() {
        match tok {
            Token::Comma | Token::LeftBracket if span.end <= param_span.start => {
                prev_sep = Some(*span);
            }
            Token::Comma if span.start >= param_span.end && next_sep.is_none() => {
                next_sep = Some(*span);
                break;
            }
            Token::RightBracket if span.start >= param_span.end && next_sep.is_none() => {
                next_sep = Some(*span);
                break;
            }
            _ => {}
        }
    }

    // Also need to include the type annotation after the param name (`: type`)
    let mut type_end = param_span.end;
    let mut seen_colon = false;
    let mut paren_depth = 0i32;
    for (tok, span) in tokens.iter() {
        if span.start < param_span.end {
            continue;
        }
        match tok {
            Token::Colon if !seen_colon => {
                seen_colon = true;
            }
            Token::Comma | Token::RightBracket if paren_depth == 0 => {
                type_end = span.start;
                break;
            }
            Token::LeftBracket => paren_depth += 1,
            Token::RightBracket => paren_depth -= 1,
            _ => {}
        }
    }

    let prev = prev_sep?;
    // If previous separator is `(`, delete from param start to next comma
    if matches!(
        tokens.iter().find(|(_, s)| s.start == prev.start).map(|(t, _)| t),
        Some(Token::LeftBracket)
    ) {
        // First parameter: delete up to next comma (or just the param if only one)
        if let Some(next) = next_sep {
            if matches!(
                tokens.iter().find(|(_, s)| s.start == next.start).map(|(t, _)| t),
                Some(Token::Comma)
            ) {
                return Some(base_db::text_range(param_span.start, next.end));
            }
        }
        return Some(base_db::text_range(param_span.start, type_end));
    }

    // Not first parameter: delete from previous comma to type end
    Some(base_db::text_range(prev.start, type_end))
}

/// Drop the argument in `param_index` from every call to `fn_name` in this
/// file.
///
/// Which calls, from the callgraph — a name match would also hit a string
/// or another definition's; where the arguments are, from the tree, which
/// is the only thing that knows.
fn call_argument_edits(
    ctx: &AssistContext<'_>,
    fn_name: &str,
    param_index: usize,
) -> Vec<TextEdit> {
    use parser::SyntaxKind as SK;

    let (Some(graph), Some(root)) = (ctx.file.callgraph(), ctx.source_file()) else {
        return Vec::new();
    };
    let mut edits = Vec::new();
    for site in graph.call_sites_in_file() {
        if site.callee != fn_name {
            continue;
        }
        let call = root
            .descendants()
            .filter(|n| {
                n.kind() == SK::CALL_EXPR
                    && usize::from(n.text_range().start()) <= site.callee_span.start
                    && site.callee_span.end <= usize::from(n.text_range().end())
            })
            .last();
        let Some(call) = call else { continue };
        // `f(1, 2)` is `CALL_EXPR[callee, ARG_LIST[1, 2]]` — the arguments
        // are that node's children, not the call's. A parenthesised
        // expression in argument position is a `TUPLE_EXPR`, so both are
        // unwrapped.
        let after_callee: Vec<_> = call
            .children()
            .filter(|c| {
                !c.kind().is_trivia() && usize::from(c.text_range().start()) >= site.callee_span.end
            })
            .collect();
        let args: Vec<_> = match after_callee.as_slice() {
            [only] if matches!(only.kind(), SK::ARG_LIST | SK::TUPLE_EXPR) => {
                only.children().filter(|c| !c.kind().is_trivia()).collect()
            }
            rest => rest.to_vec(),
        };
        let Some(arg) = args.get(param_index) else { continue };
        // With its separator: the comma before it, or the one after when it
        // is the first argument.
        let (from, to) =
            match (param_index.checked_sub(1).and_then(|i| args.get(i)), args.get(param_index + 1))
            {
                (Some(prev), _) => {
                    (usize::from(prev.text_range().end()), usize::from(arg.text_range().end()))
                }
                (None, Some(next)) => {
                    (usize::from(arg.text_range().start()), usize::from(next.text_range().start()))
                }
                (None, None) => {
                    (usize::from(arg.text_range().start()), usize::from(arg.text_range().end()))
                }
            };
        if from < to {
            edits.push(TextEdit { range: base_db::text_range(from, to), new_text: String::new() });
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{check_assist, check_assist_not_applicable};
    use expect_test::expect;

    /// With a `val` above it the assist declines: the parameter's type
    /// lives there, and this cannot rewrite the arrow.
    #[test]
    fn declines_when_a_val_would_be_left_stale() {
        check_assist_not_applicable(
            remove_unused_param,
            "val f : (int, int) -> int\nfunction f(x, $0y) = x\n",
        );
    }

    /// The call in the same file loses its argument too.
    #[test]
    fn applies() {
        check_assist(
            remove_unused_param,
            "function f(x, $0y) = x\nfunction g() = f(1, 2)\n",
            expect![[r#"
                function f(x) = x
                function g() = f(1)
            "#]],
        );
    }
}
