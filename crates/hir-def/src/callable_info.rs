//! Callable signature data types + extraction from ParsedFile.
//!
//! Canonical location: hir-def. Moved from ide-db in so
//! hir-ty can access callable metadata without depending on ide-db.

use parser::Span;
use syntax::parser_lower::{DeclKind, ParsedFile};

/// A single function/mapping parameter.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub name: String,
    pub is_implicit: bool,
}

/// Signature metadata for one callable (function, val, mapping).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallableSignature {
    pub name: String,
    pub label: String,
    pub params: Vec<Parameter>,
    pub return_type: Option<String>,
}

fn span_text<'a>(text: &'a str, span: Span) -> &'a str {
    text.get(span.start..span.end).unwrap_or("")
}

/// Extract callable signatures from a ParsedFile.
/// Pure function: no FileDb dependency.
pub fn collect_callable_signatures_from(parsed: &ParsedFile, text: &str) -> Vec<CallableSignature> {
    let mut out = Vec::new();
    for head in &parsed.callable_heads {
        if !matches!(head.kind, DeclKind::Function | DeclKind::Value | DeclKind::Mapping) {
            continue;
        }

        let label = span_text(text, head.label_span).to_string();
        let params = head
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let ty_text = param.ty_span.map(|span| span_text(text, span).to_string());
                let name = match (param.name.as_deref(), ty_text.as_deref()) {
                    (Some(name), Some(ty)) => format!("{name} : {ty}"),
                    (Some(name), None) => name.to_string(),
                    (None, Some(ty)) => format!("arg{}: {}", idx + 1, ty),
                    (None, None) => format!("arg{}", idx + 1),
                };
                Parameter { name, is_implicit: span_text(text, param.span).contains("implicit") }
            })
            .collect::<Vec<_>>();
        let return_type = head.return_type_span.map(|span| span_text(text, span).to_string());
        out.push(CallableSignature { name: head.name.clone(), label, params, return_type });
    }
    out
}
