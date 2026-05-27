//! Function parameter name completion.
//!
//! Scans other functions in the same file for parameters whose names
//! match the current prefix. Offers `name : type` suggestions.

use super::Completions;
use crate::context::CompletionContext;
use crate::CompletionItemKind;

/// Complete function parameter names based on parameters of other
/// functions in the same file.
pub(crate) fn complete_fn_param(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let prefix_lower = ctx.prefix.to_ascii_lowercase();

    // Parse the current file's function parameter names.
    let Some(parsed) = ctx.file.parsed() else {
        return;
    };

    let mut seen = std::collections::HashSet::new();

    for decl in &parsed.decls {
        if !matches!(
            decl.kind,
            syntax::parser_lower::DeclKind::Function | syntax::parser_lower::DeclKind::Mapping
        ) {
            continue;
        }

        // Extract parameters from the declaration text.
        let def_text = ctx.file.text().get(decl.span.start..decl.span.end).unwrap_or("");
        let Some(open_paren) = def_text.find('(') else {
            continue;
        };
        let Some(close_paren) = def_text[open_paren..].find(')') else {
            continue;
        };
        let params_str = &def_text[open_paren + 1..open_paren + close_paren];

        for param in params_str.split(',') {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            // Parse `name : type` or just `type`.
            let (name, type_text) = if let Some(colon_pos) = param.find(':') {
                let n = param[..colon_pos].trim();
                let t = param[colon_pos + 1..].trim();
                (n, t)
            } else {
                continue; // Type-only param, no name to suggest.
            };

            if name.is_empty() {
                continue;
            }

            // Filter by prefix.
            if !prefix_lower.is_empty() && !name.to_ascii_lowercase().starts_with(&prefix_lower) {
                continue;
            }

            let label = format!("{name} : {type_text}");
            if !seen.insert(label.clone()) {
                continue; // Deduplicate.
            }

            acc.add(crate::IdeDbCompletionItem {
                label,
                kind: CompletionItemKind::Variable,
                detail: Some(format!("param from {}", decl.name)),
                documentation: None,
                insert_text: Some(format!("{name} : {type_text}")),
                text_edit: None,
                sort_text: Some(format!("1{name}")),
                filter_text: Some(name.to_string()),
                deprecated: false,
                relevance: Default::default(),
            });
        }
    }
}
