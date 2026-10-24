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
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

/// Delegates to the existing `inline_call_edits()` implementation
/// in `lib.rs`, wrapping it in the handler pattern.
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

/// Inline a function call by replacing it with the function body.
///
/// substitutes parameters with actual arguments, and replaces the
/// call expression with the inlined body.
pub fn inline_call_edits(file: &dyn FileDb, offset: usize) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let tokens = file.tokens()?;

    // Find the function name at offset
    let (token, name_span) =
        tokens.iter().rev().find(|(_, s)| s.start <= offset && offset <= s.end)?;
    // A definition's own head is not a call. `function f(c) = match c { .. }`
    // has an `f(c)` that reads like one, and inlining there produced
    // `function match c { .. } = match c { .. }` — source that does not parse.
    let name_index = tokens.iter().position(|(_, s)| s.start == name_span.start)?;
    let preceded_by_definition_keyword =
        tokens[..name_index].iter().next_back().is_some_and(|(t, _)| {
            matches!(
                t,
                parser::Token::KwFunction
                    | parser::Token::KwVal
                    | parser::Token::KwMapping
                    | parser::Token::KwClause
                    | parser::Token::KwOverload
            )
        });
    if preceded_by_definition_keyword {
        return None;
    }
    let callee_name = match token {
        parser::Token::Id(n) => n.as_str(),
        _ => return None,
    };

    // Check it's a call (followed by `(`)
    let next = tokens.iter().find(|(_, s)| s.start >= name_span.end)?;
    if !matches!(next.0, parser::Token::LeftBracket) {
        return None;
    }

    // Find matching `)` to determine full call span
    let mut depth = 0i32;
    let mut call_end = next.1.end;
    for (tok, span) in tokens.iter().filter(|(_, s)| s.start >= next.1.start) {
        match tok {
            parser::Token::LeftBracket => depth += 1,
            parser::Token::RightBracket => {
                depth -= 1;
                if depth == 0 {
                    call_end = span.end;
                    break;
                }
            }
            _ => {}
        }
    }

    // Extract actual arguments
    let actual_args = extract_call_arg_names(text, tokens, next.1.start);

    // Find function definition in ItemTree
    let tree = file.item_tree()?;
    let entry_id = tree.top_level_items().iter().find(|id| {
        id.name(tree).as_str() == callee_name && matches!(id.def_kind(tree), hir::DefKind::Function)
    })?;

    // Extract function body from source text
    let entry_span = entry_id.span(tree);
    let def_text = text.get(entry_span.start..entry_span.end)?;
    let eq_pos = def_text.find('=')?;
    let body_text = def_text[eq_pos + 1..].trim();

    // Extract parameter names from definition
    let formal_params = extract_formal_params(def_text);

    // Substitute: replace formal param names with actual arg names
    let mut inlined = body_text.to_string();
    for (formal, actual) in formal_params.iter().zip(actual_args.iter()) {
        // Whole-word replacement
        let pattern = formal.as_str();
        let mut result = String::new();
        let mut rest = inlined.as_str();
        while let Some(pos) = rest.find(pattern) {
            let before_ok = pos == 0
                || !rest.as_bytes()[pos - 1].is_ascii_alphanumeric()
                    && rest.as_bytes()[pos - 1] != b'_';
            let after_ok = pos + pattern.len() >= rest.len()
                || !rest.as_bytes()[pos + pattern.len()].is_ascii_alphanumeric()
                    && rest.as_bytes()[pos + pattern.len()] != b'_';
            if before_ok && after_ok {
                result.push_str(&rest[..pos]);
                result.push_str(actual);
                rest = &rest[pos + pattern.len()..];
            } else {
                result.push_str(&rest[..pos + pattern.len()]);
                rest = &rest[pos + pattern.len()..];
            }
        }
        result.push_str(rest);
        inlined = result;
    }

    // Replace the call expression with the inlined body
    let call_range = base_db::text_range(name_span.start, call_end);
    Some(vec![IdeTextEdit { range: call_range, new_text: inlined }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

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
