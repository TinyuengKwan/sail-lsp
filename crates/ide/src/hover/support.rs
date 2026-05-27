use ide_db::{FileDb, LineCol};
use syntax::parser_lower::Decl;
use url::Url;

fn token_range_for_offsets(
    tokens: &[(parser::Token, parser::Span)],
    start_offset: usize,
    end_offset: usize,
) -> Option<(usize, usize)> {
    let start_idx = tokens.iter().position(|(_, s)| s.start >= start_offset);
    let end_idx = tokens.iter().rposition(|(_, s)| s.end <= end_offset).map(|i| i + 1);
    start_idx.zip(end_idx)
}

/// Infer argument types at a call site for signature help / hover.
///
/// Uses salsa-cached InferenceResult via `cached_expr_type_text`
/// instead of inline type inference.
/// queries cached `InferenceResult` (`hir/src/source_analyzer.rs:381`).
pub fn infer_call_arg_types_at_position(
    _files: &[(&Url, &dyn FileDb)],
    _current_uri: &Url,
    current_file: &dyn FileDb,
    position: LineCol,
    callee: &str,
) -> Option<Vec<Option<String>>> {
    let offset = current_file.offset_at(&position);
    let tokens = current_file.tokens()?;
    let parsed = current_file.parsed()?;
    let call = parsed.call_sites.iter().find(|call| {
        call.callee == callee && call.callee_span.start <= offset && offset <= call.callee_span.end
    })?;

    let mut arg_types = Vec::new();
    let mut current_idx = call.open_span.end;
    let mut boundary_offsets: Vec<usize> =
        call.arg_separator_spans.iter().map(|span| span.start).collect();
    if let Some(close) = call.close_span {
        boundary_offsets.push(close.start);
    }

    for boundary in boundary_offsets {
        let inferred = token_range_for_offsets(tokens, current_idx, boundary).and_then(
            |(start_idx, end_idx)| {
                let start = tokens.get(start_idx).map(|(_, s)| s.start)?;
                let end = tokens.get(end_idx.saturating_sub(1)).map(|(_, s)| s.end)?;
                let span = parser::Span::new(start, end);
                // Use salsa-cached InferenceResult instead of
                // inline inference (infer_expr_type_text_in_files).
                current_file.cached_expr_type_text(span)
            },
        );
        arg_types.push(inferred);
        current_idx = boundary + 1;
    }

    Some(arg_types)
}

fn span_text<'a>(file: &'a dyn FileDb, span: parser::Span) -> &'a str {
    file.text()[span.start..span.end].trim()
}

/// Get type hint for a binding (let/var/parameter).
///
/// Uses salsa-cached InferenceResult instead of inline type
/// inference. `type_info_of()`:
///   `sema.type_of_expr(expr)?`  → queries salsa-cached InferenceResult
///
/// RA never does inline type inference for hover — it always queries
/// the cached InferenceResult via `SourceAnalyzer::type_of_expr()`
/// (`hir/src/source_analyzer.rs:381-395`).
///
/// Path: `FileDb::binding_type_text(span)` → `SalsaFile` impl →
/// `infer(db, id)` → `TypeCheckResult::binding_type_text()`
pub fn binding_type_hint(
    _files: &[(&Url, &dyn FileDb)],
    _current_uri: &Url,
    file: &dyn FileDb,
    decl: &Decl,
) -> Option<String> {
    // 1. Salsa-cached type from per-callable InferenceResult.
    // SalsaFile::binding_type_text queries infer
    // (salsa-tracked, LRU=256) — no recomputation.
    if let Some(ty) = file.binding_type_text(decl.span) {
        return Some(ty);
    }

    // 2. Explicit type annotation from source (parsed typed_bindings).
    if let Some(parsed) = file.parsed() {
        if let Some(binding) =
            parsed.typed_bindings.iter().find(|binding| binding.name_span == decl.span)
        {
            return Some(span_text(file, binding.ty_span).to_string());
        }
    }

    // Removed inline type inference fallback
    // (infer_expr_type_text_in_files). RA never does inline inference
    // for hover — it exclusively uses salsa-cached InferenceResult.
    // The removed path caused stack overflow in debug builds.
    None
}
