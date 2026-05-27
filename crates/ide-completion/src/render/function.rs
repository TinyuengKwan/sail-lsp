//! Function completion rendering.

use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::{CompletionItem, CompletionRelevance};

use crate::config::{CallableSnippets, CompletionConfig};

/// Render a function completion item with proper snippet support.
/// Generates snippets based on the `CallableSnippets` config:
/// - `FillArguments` → `fn_name(${1:arg1}, ${2:arg2})$0`
/// - `AddParentheses` → `fn_name($0)`
/// - No config / no params → `fn_name()$0`
pub(crate) fn render_function(
    name: &str,
    params: &[(String, String)], // (name, type) pairs
    ret_ty: Option<&str>,
    signature: &str,
    config: &CompletionConfig,
) -> CompletionItem {
    let insert_text = if params.is_empty() {
        if config.snippet_cap.is_some() {
            format!("{name}()$0")
        } else {
            format!("{name}()")
        }
    } else if config.snippet_cap.is_some() {
        match config.callable {
            Some(CallableSnippets::FillArguments) => {
                let args: String = params
                    .iter()
                    .enumerate()
                    .map(|(i, (pname, _pty))| format!("${{{}:{}}}", i + 1, snippet_escape(pname)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({args})$0")
            }
            Some(CallableSnippets::AddParentheses) | None => {
                format!("{name}($0)")
            }
        }
    } else {
        format!("{name}(")
    };

    let detail = if let Some(ret) = ret_ty {
        format!("{signature} -> {ret}")
    } else {
        signature.to_string()
    };

    CompletionItem {
        label: name.to_string(),
        kind: CompletionItemKind::Function,
        detail: Some(detail),
        insert_text: Some(insert_text),
        text_edit: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        deprecated: false,
        relevance: CompletionRelevance::default(),
    }
}

fn snippet_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}")
}
