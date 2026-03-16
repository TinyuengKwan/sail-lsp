use crate::state::File;
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation, Url,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallInfo {
    pub(crate) callee: String,
    pub(crate) callee_span: sail_parser::Span,
    pub(crate) arg_index: usize,
    pub(crate) arg_count: usize,
}

pub(crate) fn call_arg_count(call: &sail_parser::CallSite) -> usize {
    if call.open_span.end == call.close_span.map(|span| span.start).unwrap_or(0) {
        0
    } else {
        call.arg_separator_spans.len() + 1
    }
}

fn fallback_call_at_offset(file: &File, offset: usize) -> Option<CallInfo> {
    let parsed = file.parsed()?;
    let mut candidate = None::<sail_parser::CallSite>;
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
    let arg_index = call
        .arg_separator_spans
        .iter()
        .filter(|span| span.start < offset)
        .count();
    let arg_count = call_arg_count(&call);
    Some(CallInfo {
        callee: call.callee,
        callee_span: call.callee_span,
        arg_index,
        arg_count,
    })
}

pub(crate) fn ast_call_at_position<'a>(
    file: &'a File,
    position: Position,
) -> Option<sail_parser::CallAtOffset<'a>> {
    let offset = file.source.offset_at(&position);
    let ast = file.core_ast()?;
    sail_parser::find_call_at_offset(ast, offset)
}

pub(crate) fn call_info_at_position(file: &File, position: Position) -> Option<CallInfo> {
    if let Some(call) = ast_call_at_position(file, position) {
        return Some(CallInfo {
            callee: call.callee.to_string(),
            callee_span: call.callee_span,
            arg_index: call.arg_index,
            arg_count: call.args.len(),
        });
    }

    let offset = file.source.offset_at(&position);
    fallback_call_at_offset(file, offset)
}

pub(crate) fn find_call_at_position(file: &File, position: Position) -> Option<(String, usize)> {
    let call = call_info_at_position(file, position)?;
    Some((call.callee, call.arg_index))
}

pub(crate) fn signature_help_for_position<'a, I>(
    files: I,
    uri: &Url,
    file: &File,
    position: Position,
) -> Option<SignatureHelp>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let (callee, arg_index) = find_call_at_position(file, position)?;
    let all_files = files.into_iter().collect::<Vec<_>>();
    let sig = super::analysis::find_callable_signature(all_files.iter().copied(), uri, &callee)?;

    let mut documentation = None;
    for (_, candidate_file) in all_files {
        if let Some(parsed) = candidate_file.parsed() {
            if let Some(decl) = parsed
                .decls
                .iter()
                .find(|d| d.name == callee && d.scope == sail_parser::Scope::TopLevel)
            {
                if let Some(comments) =
                    super::analysis::extract_comments(candidate_file.source.text(), decl.span.start)
                {
                    documentation = Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: comments,
                    }));
                }
                break;
            }
        }
    }

    let active_parameter = arg_index
        .min(sig.params.len().saturating_sub(1))
        .try_into()
        .unwrap_or(0);

    let signature_information = SignatureInformation {
        label: sig.label,
        documentation,
        parameters: Some(
            sig.params
                .iter()
                .map(|param| ParameterInformation {
                    label: ParameterLabel::Simple(param.name.clone()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: Some(active_parameter),
    };

    Some(SignatureHelp {
        signatures: vec![signature_information],
        active_signature: Some(0),
        active_parameter: Some(active_parameter),
    })
}
