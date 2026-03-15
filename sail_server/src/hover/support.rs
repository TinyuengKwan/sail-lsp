use crate::state::File;
use crate::symbols::{ast_call_at_position, find_callable_signature};
use sail_parser::{Decl, Expr, Literal};
use tower_lsp::lsp_types::{Position, Url};

fn span_text(file: &File, span: sail_parser::Span) -> String {
    file.source.text()[span.start..span.end].trim().to_string()
}

fn binding_type_text_before_offset(file: &File, name: &str, offset: usize) -> Option<String> {
    let parsed = file.parsed()?;
    parsed
        .typed_bindings
        .iter()
        .filter(|binding| binding.name == name && binding.name_span.start <= offset)
        .max_by_key(|binding| binding.name_span.start)
        .map(|binding| span_text(file, binding.ty_span))
}

fn literal_type_text(literal: &Literal) -> Option<String> {
    match literal {
        Literal::Bool(_) => Some("bool".to_string()),
        Literal::Unit => Some("unit".to_string()),
        Literal::Number(text) => {
            if text.contains('.') {
                Some("real".to_string())
            } else {
                Some("int".to_string())
            }
        }
        Literal::Binary(text) => {
            let len = text
                .trim_start_matches("0b")
                .chars()
                .filter(|ch| *ch != '_')
                .count();
            let ty = match len {
                0..=8 => "bits(8)",
                9..=16 => "bits(16)",
                17..=32 => "bits(32)",
                33..=64 => "bits(64)",
                _ => "bits",
            };
            Some(ty.to_string())
        }
        Literal::Hex(text) => {
            let len = text
                .trim_start_matches("0x")
                .chars()
                .filter(|ch| *ch != '_')
                .count();
            let ty = match len {
                0..=2 => "bits(8)",
                3..=4 => "bits(16)",
                5..=8 => "bits(32)",
                9..=16 => "bits(64)",
                _ => "bits",
            };
            Some(ty.to_string())
        }
        Literal::String(_) => Some("string".to_string()),
        Literal::Undefined | Literal::BitZero | Literal::BitOne => None,
    }
}

fn expr_symbol(expr: &(Expr, sail_parser::Span)) -> Option<&str> {
    match &expr.0 {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Field { field, .. } => Some(field.0.as_str()),
        _ => None,
    }
}

fn token_range_for_offsets(
    tokens: &[(sail_parser::Token, sail_parser::Span)],
    start_offset: usize,
    end_offset: usize,
) -> Option<(usize, usize)> {
    let mut start_idx = None;
    let mut end_idx = None;

    for (idx, (_, span)) in tokens.iter().enumerate() {
        if span.start >= start_offset && span.end <= end_offset {
            start_idx.get_or_insert(idx);
            end_idx = Some(idx);
        }
    }

    start_idx.zip(end_idx)
}

pub(crate) fn infer_expr_type_text<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    expr: &(Expr, sail_parser::Span),
) -> Option<String> {
    match &expr.0 {
        Expr::Attribute { expr, .. }
        | Expr::Prefix { expr, .. }
        | Expr::Field { expr, .. }
        | Expr::Index { expr, .. }
        | Expr::Slice { expr, .. }
        | Expr::Exit(Some(expr))
        | Expr::VectorSlice { expr, .. } => {
            infer_expr_type_text(files, current_uri, current_file, expr)
        }
        Expr::Assert { .. } => Some("unit".to_string()),
        Expr::Exit(None) => None,
        Expr::Cast { ty, .. } => Some(span_text(current_file, ty.1)),
        Expr::Literal(literal) => literal_type_text(literal),
        Expr::Ident(name) => binding_type_text_before_offset(current_file, name, expr.1.start)
            .or_else(|| {
                find_callable_signature(files.iter().copied(), current_uri, name)
                    .and_then(|sig| sig.return_type)
            }),
        Expr::Call(call) => expr_symbol(&call.callee).and_then(|name| {
            find_callable_signature(files.iter().copied(), current_uri, name)
                .and_then(|sig| sig.return_type)
        }),
        Expr::SizeOf(_) => Some("int".to_string()),
        _ => None,
    }
}

pub(crate) fn infer_call_arg_types_at_position<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    position: Position,
    callee: &str,
) -> Option<Vec<Option<String>>> {
    if let Some(call) =
        ast_call_at_position(current_file, position).filter(|call| call.callee == callee)
    {
        return Some(
            call.args
                .iter()
                .map(|arg| infer_expr_type_text(files, current_uri, current_file, arg))
                .collect(),
        );
    }

    let offset = current_file.source.offset_at(&position);
    let tokens = current_file.tokens.as_ref()?;
    let parsed = current_file.parsed()?;
    let call = parsed.call_sites.iter().find(|call| {
        call.callee == callee && call.callee_span.start <= offset && offset <= call.callee_span.end
    })?;

    let mut arg_types = Vec::new();
    let mut current_idx = call.open_span.end;
    let mut boundary_offsets: Vec<usize> = call
        .arg_separator_spans
        .iter()
        .map(|span| span.start)
        .collect();
    if let Some(close) = call.close_span {
        boundary_offsets.push(close.start);
    }

    for boundary in boundary_offsets {
        let inferred = token_range_for_offsets(tokens, current_idx, boundary)
            .and_then(|(start_idx, end_idx)| {
                sail_parser::parse_expr_fragment(tokens, start_idx, end_idx)
            })
            .and_then(|expr| infer_expr_type_text(files, current_uri, current_file, &expr));
        arg_types.push(inferred);
        current_idx = boundary + 1;
    }

    Some(arg_types)
}

pub(crate) fn binding_type_hint<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    file: &File,
    decl: &Decl,
) -> Option<String> {
    if let Some(parsed) = file.parsed() {
        if let Some(binding) = parsed
            .typed_bindings
            .iter()
            .find(|binding| binding.name_span == decl.span)
        {
            return Some(span_text(file, binding.ty_span));
        }
    }

    let ast = file.ast()?;
    let binding = sail_parser::find_binding_value_at_span(ast, decl.span)?;
    if let Some(ty_span) = binding.explicit_ty {
        return Some(span_text(file, ty_span));
    }

    infer_expr_type_text(files, current_uri, file, binding.value)
}
