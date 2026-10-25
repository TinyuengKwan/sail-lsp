//! Keyword completion provider.
//!
//! Provides context-aware keyword suggestions (top-level vs local).

use super::Completions;
use crate::context::{CompletionContext, CompletionPosition};

/// Complete keywords based on position context.
/// `complete_for_and_where(acc, ctx, item)`.
pub(crate) fn complete_keywords(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    keywords: &[&str],
) {
    if !ctx.config.enable_keyword_completions {
        return;
    }
    let prefix_lower = ctx.prefix_lower();

    for keyword in keywords {
        if !prefix_lower.is_empty() && !keyword.to_ascii_lowercase().starts_with(&prefix_lower) {
            continue;
        }

        let is_top_level_kw = matches!(
            *keyword,
            "function"
                | "val"
                | "enum"
                | "struct"
                | "union"
                | "type"
                | "register"
                | "overload"
                | "bitfield"
                | "newtype"
                | "mapping"
                | "scattered"
                | "default"
                | "end"
        );
        let is_local_kw = matches!(
            *keyword,
            "let"
                | "var"
                | "if"
                | "else"
                | "match"
                | "return"
                | "foreach"
                | "while"
                | "throw"
                | "exit"
                | "assert"
                | "try"
                | "repeat"
                | "do"
                | "then"
                | "in"
        );

        let include = match ctx.position {
            CompletionPosition::TopLevel => is_top_level_kw || !is_local_kw,
            CompletionPosition::Expression | CompletionPosition::Pattern => {
                is_local_kw || !is_top_level_kw
            }
            CompletionPosition::TypeAnnotation => false, // No keywords in type position
        };

        if !include {
            continue;
        }

        let snippet = ctx
            .config
            .enable_snippets
            .then(|| keyword_snippet(keyword, ctx.is_top_level))
            .flatten();

        acc.add(crate::render::render_keyword_snippet(keyword, snippet));
    }
}

fn keyword_snippet(keyword: &str, is_top_level: bool) -> Option<String> {
    match keyword {
        "foreach" if !is_top_level => {
            Some("foreach (${1:i} from ${2:0} to ${3:n}) {\n\t$0\n}".into())
        }
        "if" if !is_top_level => Some("if ${1:condition} then {\n\t$0\n}".into()),
        "match" if !is_top_level => Some("match ${1:x} {\n\t${2:case} => $0\n}".into()),
        "while" if !is_top_level => Some("while ${1:condition} do {\n\t$0\n}".into()),
        "let" if !is_top_level => Some("let ${1:x} = $0".into()),
        "var" if !is_top_level => Some("var ${1:x} = $0".into()),
        "try" if !is_top_level => Some("try {\n\t$0\n} catch {\n\t${1:_} => ()\n}".into()),
        "function" if is_top_level => Some("function ${1:name}(${2:args}) = $0".into()),
        "val" if is_top_level => Some("val ${1:name} : $0".into()),
        "struct" if is_top_level => Some("struct ${1:name} = {\n\t${2:field} : $0\n}".into()),
        "enum" if is_top_level => Some("enum ${1:name} = { $0 }".into()),
        "union" if is_top_level => Some("union ${1:name} = {\n\t${2:Variant} : $0\n}".into()),
        "register" if is_top_level => Some("register ${1:name} : $0".into()),
        _ => None,
    }
}
