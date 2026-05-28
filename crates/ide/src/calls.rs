use ide_db::{extract_comments, FileDb, LineCol};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallInfo {
    pub callee: String,
    pub callee_span: parser::Span,
    pub arg_index: usize,
    pub arg_count: usize,
}

pub fn call_arg_count(call: &syntax::parser_lower::CallSite) -> usize {
    if call.open_span.end == call.close_span.map(|span| span.start).unwrap_or(0) {
        0
    } else {
        let base = call.arg_separator_spans.len() + 1;
        // Detect trailing comma: last separator is adjacent to close paren
        if let (Some(last_sep), Some(close)) = (call.arg_separator_spans.last(), call.close_span) {
            if !call.arg_separator_spans.is_empty() && last_sep.end >= close.start.saturating_sub(1) {
                return base - 1;
            }
        }
        base
    }
}

fn fallback_call_at_offset(file: &dyn FileDb, offset: usize) -> Option<CallInfo> {
    let parsed = file.parsed()?;
    let mut candidate = None::<syntax::parser_lower::CallSite>;
    for call in &parsed.call_sites {
        if call.callee_span.start > offset {
            continue;
        }
        if let Some(close) = call.close_span {
            if close.end < offset {
                continue;
            }
        }
        match &candidate {
            Some(current) if current.callee_span.start > call.callee_span.start => {}
            _ => candidate = Some(call.clone()),
        }
    }
    let call = candidate?;
    let arg_index = call.arg_separator_spans.iter().filter(|span| span.start < offset).count();
    let arg_count = call_arg_count(&call);
    Some(CallInfo { callee: call.callee, callee_span: call.callee_span, arg_index, arg_count })
}

pub fn call_info_at_position(file: &dyn FileDb, position: LineCol) -> Option<CallInfo> {
    let offset = file.offset_at(&position);
    fallback_call_at_offset(file, offset)
}

pub fn find_call_at_position(file: &dyn FileDb, position: LineCol) -> Option<(String, usize)> {
    let call = call_info_at_position(file, position)?;
    Some((call.callee, call.arg_index))
}

/// Signature help — returns internal SignatureHelp (framework-independent).
pub fn signature_help_ide(
    files: &[(&Url, &dyn FileDb)],
    _uri: &Url,
    file: &dyn FileDb,
    position: LineCol,
) -> Option<ide_db::ide_types::SignatureHelp> {
    let (callee, arg_index) = find_call_at_position(file, position)?;
    let all_files = files.to_vec();

    // Find ALL overloaded signatures (not just the first match)
    let all_sigs =
        ide_db::symbol_index::find_all_callable_signatures(all_files.iter().copied(), &callee);
    if all_sigs.is_empty() {
        return None;
    }

    // Extract documentation from the first declaration found
    let mut documentation = None;
    for (_, candidate_file) in &all_files {
        if let Some(parsed) = candidate_file.parsed() {
            if let Some(decl) = parsed
                .decls
                .iter()
                .find(|d| d.name == callee && d.scope == syntax::parser_lower::Scope::TopLevel)
            {
                if let Some(comments) = extract_comments(candidate_file.text(), decl.span.start) {
                    documentation = Some(comments);
                }
                break;
            }
        }
    }

    // Build signature info for each overload
    let mut signatures = Vec::new();
    let mut best_active = 0usize;
    let mut best_score = 0usize;

    for (sig_idx, sig) in all_sigs.iter().enumerate() {
        // Score this overload: prefer one where the arg count matches
        let non_implicit_count = sig.params.iter().filter(|p| !p.is_implicit).count();
        let score = if non_implicit_count > arg_index { non_implicit_count } else { 0 };
        if score > best_score {
            best_score = score;
            best_active = sig_idx;
        }

        signatures.push(ide_db::ide_types::SignatureInfo {
            label: sig.label.clone(),
            documentation: if sig_idx == 0 { documentation.clone() } else { None },
            parameters: sig
                .params
                .iter()
                .map(|param| ide_db::ide_types::ParameterInfo { label: param.name.clone() })
                .collect(),
        });
    }

    let active_parameter = {
        let sig = &all_sigs[best_active];
        let mut visible_idx = 0;
        let mut full_idx = 0;
        for param in &sig.params {
            if !param.is_implicit {
                if visible_idx == arg_index {
                    break;
                }
                visible_idx += 1;
            }
            full_idx += 1;
        }
        full_idx.min(sig.params.len().saturating_sub(1))
    };

    Some(ide_db::ide_types::SignatureHelp {
        signatures,
        active_signature: Some(best_active),
        active_parameter: Some(active_parameter),
    })
}
