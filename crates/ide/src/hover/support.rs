use ide_db::FileDb;
use syntax::parser_lower::Decl;
use url::Url;

fn span_text(file: &dyn FileDb, span: parser::Span) -> &str {
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
/// Path: `Semantics::binding_type_text` → `SourceAnalyzer` →
/// `infer(db, id)` → `TypeCheckResult::binding_type_text()`
pub fn binding_type_hint(
    _files: &[(&Url, &dyn FileDb)],
    _current_uri: &Url,
    file: &dyn FileDb,
    decl: &Decl,
) -> Option<String> {
    // 1. Salsa-cached type from per-callable InferenceResult.
    // Semantics::binding_type_text queries infer
    // (salsa-tracked, LRU=256) — no recomputation.
    if let Some(ty) = file
        .file_text()
        .and_then(|(db, ft)| hir::Semantics::new(db).binding_type_display(ft, decl.span))
    {
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
