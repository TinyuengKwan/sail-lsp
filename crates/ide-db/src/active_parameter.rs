//! Active parameter detection for signature help.
//! Sail adaptation: uses ParsedFile.call_sites for call detection
//! and SignatureIndex for parameter resolution.

use crate::FileDb;
use parser::Span;

/// Information about the active parameter at cursor position.
#[derive(Debug, Clone)]
pub struct ActiveParameter {
    /// Parameter name (from val spec / function signature).
    pub name: String,
    /// Zero-based index among non-implicit parameters.
    pub index: usize,
    /// Total parameter count (non-implicit).
    pub count: usize,
    /// Whether this parameter is implicit (forall-quantified).
    pub is_implicit: bool,
}

/// Information about the callable at cursor position.
#[derive(Debug, Clone)]
pub struct ActiveCallable {
    /// Name of the function/mapping being called.
    pub name: String,
    /// Span of the callee identifier.
    pub name_span: Span,
    /// Active parameter (if cursor is within arguments).
    pub active_param: Option<ActiveParameter>,
}

/// Detect the callable and active parameter at a byte offset.
///
/// `callable_for_token(sema, token) -> Option<(Callable, Option<usize>)>`
/// Sail adaptation: uses ParsedFile.call_sites + comma counting.
pub fn active_callable(file: &dyn FileDb, offset: usize) -> Option<ActiveCallable> {
    let parsed = file.parsed()?;

    // Find the innermost call site containing the offset
    let mut best: Option<&syntax::parser_lower::CallSite> = None;
    for call in &parsed.call_sites {
        if call.callee_span.start > offset {
            continue;
        }
        if let Some(close) = call.close_span {
            if close.end < offset {
                continue;
            }
        }
        match &best {
            Some(current) if current.callee_span.start > call.callee_span.start => {}
            _ => best = Some(call),
        }
    }
    let call = best?;

    // Comma counting: number of commas before offset = argument index
    // `arg_list.children_with_tokens().filter_map(into_comma).take_while(|t| t.start() <= offset).count()`
    let arg_index = call.arg_separator_spans.iter().filter(|span| span.start < offset).count();

    // Resolve parameter info from SignatureIndex
    let active_param = if let Some(sig_index) = file.signature_index() {
        if let Some(sig) = sig_index.get(&call.callee) {
            let non_implicit: Vec<_> = sig.params.iter().filter(|p| !p.is_implicit).collect();
            let count = non_implicit.len();
            let param = non_implicit.get(arg_index).map(|p| ActiveParameter {
                name: p.name.clone(),
                index: arg_index,
                count,
                is_implicit: false,
            });
            param
        } else {
            None
        }
    } else {
        None
    };

    Some(ActiveCallable { name: call.callee.clone(), name_span: call.callee_span, active_param })
}

#[cfg(test)]
mod tests {
    // Integration tests require FileDb implementation — covered in sail-lsp/tests.rs
}
