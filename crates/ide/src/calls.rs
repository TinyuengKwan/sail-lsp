use ide_db::FileDb;
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
            if !call.arg_separator_spans.is_empty() && last_sep.end >= close.start.saturating_sub(1)
            {
                return base - 1;
            }
        }
        base
    }
}

pub(crate) fn fallback_call_at_offset(file: &dyn FileDb, offset: usize) -> Option<CallInfo> {
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
