use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use chumsky::Parser;
use sail_parser::{
    Attribute, AttributeData, AttributeEntry, BitfieldField, BlockItem, CallableClause,
    CallableDef, ConstraintDef, DirectiveDef, EnumFunction, EnumMember, Expr, ExternBinding,
    ExternSpec, FieldExpr, FieldPattern, LetBinding, LoopMeasure, MappingBody, MatchCase, NamedDef,
    NamedDefDetail, Pattern, RecMeasure, ScatteredClauseDef, ScatteredDef, SourceFile, Span,
    TerminationMeasureDef, TerminationMeasureKind, TopLevelDef, TypeAliasDef, TypeExpr, TypeParam,
    TypeParamSpec, TypeParamTail, TypedField, UnionPayload, UnionVariant, VectorUpdate,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ErrorKind {
    Type,
    Pattern,
    Expr,
}

#[derive(Clone, Debug, Default)]
struct SampleBucket {
    count: usize,
    samples: Vec<String>,
}

fn main() {
    let roots: Vec<PathBuf> = env::args().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        eprintln!("usage: cargo run -p sail_parser --example error_samples -- <file-or-dir>...");
        std::process::exit(2);
    }

    let mut files = Vec::new();
    for root in &roots {
        collect_sail_files(root, &mut files);
    }
    files.sort();
    files.dedup();

    let mut total_files = 0usize;
    let mut lex_failures = 0usize;
    let mut parse_failures = 0usize;
    let mut buckets: BTreeMap<(ErrorKind, String), SampleBucket> = BTreeMap::new();

    for path in files {
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        total_files += 1;

        let Ok(tokens) = sail_parser::lexer().parse(source.as_str()).into_result() else {
            lex_failures += 1;
            continue;
        };
        let Ok(ast) = sail_parser::parse_source(&tokens).into_result() else {
            parse_failures += 1;
            continue;
        };
        collect_source_file_errors(&ast, &source, &path, &mut buckets);
    }

    let total_errors: usize = buckets.values().map(|bucket| bucket.count).sum();
    let parsed_files = total_files.saturating_sub(lex_failures + parse_failures);
    println!("files_scanned: {total_files}");
    println!("lex_failures: {lex_failures}");
    println!("parse_failures: {parse_failures}");
    println!("parsed_files: {parsed_files}");
    println!("distinct_error_shapes: {}", buckets.len());
    println!("total_error_nodes: {total_errors}");

    for ((kind, snippet), bucket) in buckets
        .iter()
        .filter(|(_, bucket)| bucket.count > 0)
        .rev()
        .take(80)
    {
        println!();
        println!("[{kind:?}] {} hits", bucket.count);
        println!("  snippet: {snippet}");
        for sample in &bucket.samples {
            println!("  sample: {sample}");
        }
    }
}

fn collect_sail_files(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.is_file() {
        if path.extension().is_some_and(|ext| ext == "sail") {
            out.push(path.to_path_buf());
        }
        return;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_sail_files(&entry.path(), out);
    }
}

fn record_error(
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
    kind: ErrorKind,
    source: &str,
    path: &Path,
    span: Span,
) {
    let snippet = snippet_for_span(source, span);
    let sample = format!(
        "{}:{}:{}: {}",
        path.display(),
        line_col(source, span.start).0,
        line_col(source, span.start).1,
        snippet
    );
    let bucket = buckets.entry((kind, snippet)).or_default();
    bucket.count += 1;
    if bucket.samples.len() < 3 && !bucket.samples.iter().any(|existing| existing == &sample) {
        bucket.samples.push(sample);
    }
}

fn snippet_for_span(source: &str, span: Span) -> String {
    if span.start >= span.end || span.end > source.len() {
        return "<empty>".to_string();
    }
    let raw = &source[span.start..span.end];
    normalize_snippet(raw)
}

fn normalize_snippet(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        "<empty>".to_string()
    } else if trimmed.len() > 120 {
        format!("{}...", &trimmed[..120])
    } else {
        trimmed.to_string()
    }
}

fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let capped = offset.min(source.len());
    let prefix = &source[..capped];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let col = prefix
        .rsplit('\n')
        .next()
        .map(|line| line.chars().count() + 1)
        .unwrap_or(1);
    (line, col)
}

fn collect_source_file_errors(
    ast: &SourceFile,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    for (item, _) in &ast.items {
        collect_top_level_errors(item, source, path, buckets);
    }
}

fn collect_top_level_errors(
    item: &TopLevelDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match item {
        TopLevelDef::Scattered(def) => collect_scattered_errors(def, source, path, buckets),
        TopLevelDef::ScatteredClause(def) => {
            collect_scattered_clause_errors(def, source, path, buckets)
        }
        TopLevelDef::CallableSpec(spec) => {
            collect_type_expr_errors(&spec.signature, source, path, buckets);
            if let Some(externs) = &spec.externs {
                collect_extern_spec_errors(&externs.0, source, path, buckets);
            }
        }
        TopLevelDef::CallableDef(def) => collect_callable_def_errors(def, source, path, buckets),
        TopLevelDef::TypeAlias(def) => collect_type_alias_errors(def, source, path, buckets),
        TopLevelDef::Named(def) => collect_named_def_errors(def, source, path, buckets),
        TopLevelDef::Default(_) | TopLevelDef::Fixity(_) | TopLevelDef::Instantiation(_) => {}
        TopLevelDef::Directive(def) => collect_directive_errors(def, source, path, buckets),
        TopLevelDef::End(_) => {}
        TopLevelDef::Constraint(def) => collect_constraint_errors(def, source, path, buckets),
        TopLevelDef::TerminationMeasure(def) => {
            collect_termination_measure_errors(def, source, path, buckets)
        }
    }
}

fn collect_scattered_errors(
    def: &ScatteredDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(params) = &def.params {
        collect_type_param_spec_errors(&params.0, source, path, buckets);
    }
    if let Some(signature) = &def.signature {
        collect_type_expr_errors(signature, source, path, buckets);
    }
}

fn collect_scattered_clause_errors(
    def: &ScatteredClauseDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(ty) = &def.ty {
        collect_type_expr_errors(ty, source, path, buckets);
    }
}

fn collect_callable_def_errors(
    def: &CallableDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(signature) = &def.signature {
        collect_type_expr_errors(signature, source, path, buckets);
    }
    if let Some(rec_measure) = &def.rec_measure {
        collect_rec_measure_errors(&rec_measure.0, source, path, buckets);
    }
    for clause in &def.clauses {
        collect_callable_clause_errors(&clause.0, source, path, buckets);
    }
    for param in &def.params {
        collect_pattern_errors(param, source, path, buckets);
    }
    if let Some(ret) = &def.return_ty {
        collect_type_expr_errors(ret, source, path, buckets);
    }
    if let Some(body) = &def.body {
        collect_expr_errors(body, source, path, buckets);
    }
    if let Some(mapping_body) = &def.mapping_body {
        collect_mapping_body_errors(mapping_body, source, path, buckets);
    }
}

fn collect_callable_clause_errors(
    clause: &CallableClause,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&clause.modifiers, source, path, buckets);
    for pattern in &clause.patterns {
        collect_pattern_errors(pattern, source, path, buckets);
    }
    if let Some(guard) = &clause.guard {
        collect_expr_errors(guard, source, path, buckets);
    }
    if let Some(quantifier) = &clause.quantifier {
        for var in &quantifier.vars {
            if let Some(kind) = &var.kind {
                if !kind.0.is_empty() {}
            }
        }
        if let Some(constraint) = &quantifier.constraint {
            collect_type_expr_errors(constraint, source, path, buckets);
        }
    }
    if let Some(ret) = &clause.return_ty {
        collect_type_expr_errors(ret, source, path, buckets);
    }
    if let Some(body) = &clause.body {
        collect_expr_errors(body, source, path, buckets);
    }
    if let Some(mapping_body) = &clause.mapping_body {
        collect_mapping_body_errors(mapping_body, source, path, buckets);
    }
}

fn collect_type_alias_errors(
    def: &TypeAliasDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(params) = &def.params {
        collect_type_param_spec_errors(&params.0, source, path, buckets);
    }
    if let Some(target) = &def.target {
        collect_type_expr_errors(target, source, path, buckets);
    }
}

fn collect_named_def_errors(
    def: &NamedDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(params) = &def.params {
        collect_type_param_spec_errors(&params.0, source, path, buckets);
    }
    if let Some(ty) = &def.ty {
        collect_type_expr_errors(ty, source, path, buckets);
    }
    if let Some(detail) = &def.detail {
        collect_named_detail_errors(detail, source, path, buckets);
    }
    if let Some(value) = &def.value {
        collect_expr_errors(value, source, path, buckets);
    }
}

fn collect_named_detail_errors(
    detail: &NamedDefDetail,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match detail {
        NamedDefDetail::Enum { members, functions } => {
            for member in members {
                collect_enum_member_errors(&member.0, source, path, buckets);
            }
            for function in functions {
                collect_enum_function_errors(&function.0, source, path, buckets);
            }
        }
        NamedDefDetail::Struct { fields } => {
            for field in fields {
                collect_typed_field_errors(&field.0, source, path, buckets);
            }
        }
        NamedDefDetail::Union { variants } => {
            for variant in variants {
                collect_union_variant_errors(&variant.0, source, path, buckets);
            }
        }
        NamedDefDetail::Bitfield { fields } => {
            for field in fields {
                collect_bitfield_field_errors(&field.0, source, path, buckets);
            }
        }
    }
}

fn collect_modifiers_errors(
    modifiers: &sail_parser::DefModifiers,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    for attr in &modifiers.attrs {
        collect_attribute_errors(&attr.0, source, path, buckets);
    }
}

fn collect_attribute_errors(
    attr: &Attribute,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    if let Some(parsed_data) = &attr.parsed_data {
        collect_attribute_data_errors(&parsed_data.0, source, path, buckets);
    }
}

fn collect_attribute_data_errors(
    data: &AttributeData,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match data {
        AttributeData::Object(entries) => {
            for entry in entries {
                collect_attribute_entry_errors(&entry.0, source, path, buckets);
            }
        }
        AttributeData::Array(items) => {
            for item in items {
                collect_attribute_data_errors(&item.0, source, path, buckets);
            }
        }
        AttributeData::Number(_)
        | AttributeData::String(_)
        | AttributeData::Ident(_)
        | AttributeData::Bool(_) => {}
    }
}

fn collect_attribute_entry_errors(
    entry: &AttributeEntry,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_attribute_data_errors(&entry.value.0, source, path, buckets);
}

fn collect_extern_spec_errors(
    spec: &ExternSpec,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match spec {
        ExternSpec::String { .. } => {}
        ExternSpec::Bindings { bindings, .. } => {
            for binding in bindings {
                collect_extern_binding_errors(&binding.0, source, path, buckets);
            }
        }
    }
}

fn collect_extern_binding_errors(
    _binding: &ExternBinding,
    _source: &str,
    _path: &Path,
    _buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
}

fn collect_type_param_spec_errors(
    spec: &TypeParamSpec,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    for param in &spec.params {
        collect_type_param_errors(param, source, path, buckets);
    }
    if let Some(tail) = &spec.tail {
        match tail {
            TypeParamTail::Type(ty) | TypeParamTail::Constraint(ty) => {
                collect_type_expr_errors(ty, source, path, buckets);
            }
        }
    }
}

fn collect_type_param_errors(
    param: &TypeParam,
    _source: &str,
    _path: &Path,
    _buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    let _ = param;
}

fn collect_enum_member_errors(
    member: &EnumMember,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    if let Some(value) = &member.value {
        collect_expr_errors(value, source, path, buckets);
    }
}

fn collect_enum_function_errors(
    function: &EnumFunction,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_type_expr_errors(&function.ty, source, path, buckets);
}

fn collect_typed_field_errors(
    field: &TypedField,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_type_expr_errors(&field.ty, source, path, buckets);
}

fn collect_union_variant_errors(
    variant: &UnionVariant,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match &variant.payload {
        UnionPayload::Type(ty) => collect_type_expr_errors(ty, source, path, buckets),
        UnionPayload::Struct { fields } => {
            for field in fields {
                collect_typed_field_errors(&field.0, source, path, buckets);
            }
        }
    }
}

fn collect_bitfield_field_errors(
    field: &BitfieldField,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_type_expr_errors(&field.high, source, path, buckets);
    if let Some(low) = &field.low {
        collect_type_expr_errors(low, source, path, buckets);
    }
}

fn collect_directive_errors(
    def: &DirectiveDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    if let Some(payload) = &def.structured_payload {
        collect_attribute_data_errors(&payload.0, source, path, buckets);
    }
}

fn collect_constraint_errors(
    def: &ConstraintDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    collect_type_expr_errors(&def.ty, source, path, buckets);
}

fn collect_termination_measure_errors(
    def: &TerminationMeasureDef,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_modifiers_errors(&def.modifiers, source, path, buckets);
    match &def.kind {
        TerminationMeasureKind::Function { pattern, body } => {
            collect_pattern_errors(pattern, source, path, buckets);
            collect_expr_errors(body, source, path, buckets);
        }
        TerminationMeasureKind::Loop { measures } => {
            for measure in measures {
                collect_loop_measure_errors(&measure.0, source, path, buckets);
            }
        }
    }
}

fn collect_loop_measure_errors(
    measure: &LoopMeasure,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match measure {
        LoopMeasure::Until(expr) | LoopMeasure::Repeat(expr) | LoopMeasure::While(expr) => {
            collect_expr_errors(expr, source, path, buckets);
        }
    }
}

fn collect_rec_measure_errors(
    measure: &RecMeasure,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_pattern_errors(&measure.pattern, source, path, buckets);
    collect_expr_errors(&measure.body, source, path, buckets);
}

fn collect_mapping_body_errors(
    body: &MappingBody,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    for arm in &body.arms {
        collect_expr_errors(&arm.0.lhs, source, path, buckets);
        collect_expr_errors(&arm.0.rhs, source, path, buckets);
        if let Some(pattern) = &arm.0.lhs_pattern {
            collect_pattern_errors(pattern, source, path, buckets);
        }
        if let Some(pattern) = &arm.0.rhs_pattern {
            collect_pattern_errors(pattern, source, path, buckets);
        }
        if let Some(guard) = &arm.0.guard {
            collect_expr_errors(guard, source, path, buckets);
        }
    }
}

fn collect_type_expr_errors(
    ty: &(TypeExpr, Span),
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match &ty.0 {
        TypeExpr::Error(_) => record_error(buckets, ErrorKind::Type, source, path, ty.1),
        TypeExpr::Forall {
            constraint, body, ..
        } => {
            if let Some(constraint) = constraint {
                collect_type_expr_errors(constraint, source, path, buckets);
            }
            collect_type_expr_errors(body, source, path, buckets);
        }
        TypeExpr::Existential {
            constraint, body, ..
        } => {
            if let Some(constraint) = constraint {
                collect_type_expr_errors(constraint, source, path, buckets);
            }
            collect_type_expr_errors(body, source, path, buckets);
        }
        TypeExpr::Effect { ty, .. } | TypeExpr::Register(ty) | TypeExpr::Prefix { ty, .. } => {
            collect_type_expr_errors(ty, source, path, buckets)
        }
        TypeExpr::App { args, .. } | TypeExpr::Tuple(args) | TypeExpr::Set(args) => {
            for arg in args {
                collect_type_expr_errors(arg, source, path, buckets);
            }
        }
        TypeExpr::Infix { lhs, rhs, .. } => {
            collect_type_expr_errors(lhs, source, path, buckets);
            collect_type_expr_errors(rhs, source, path, buckets);
        }
        TypeExpr::Conditional {
            cond,
            then_ty,
            else_ty,
        } => {
            collect_type_expr_errors(cond, source, path, buckets);
            collect_type_expr_errors(then_ty, source, path, buckets);
            collect_type_expr_errors(else_ty, source, path, buckets);
        }
        TypeExpr::Arrow { params, ret, .. } => {
            for param in params {
                collect_type_expr_errors(param, source, path, buckets);
            }
            collect_type_expr_errors(ret, source, path, buckets);
        }
        TypeExpr::Wild
        | TypeExpr::Named(_)
        | TypeExpr::TypeVar(_)
        | TypeExpr::Literal(_)
        | TypeExpr::Dec
        | TypeExpr::Inc
        | TypeExpr::Config(_) => {}
    }
}

fn collect_pattern_errors(
    pattern: &(Pattern, Span),
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match &pattern.0 {
        Pattern::Error(_) => record_error(buckets, ErrorKind::Pattern, source, path, pattern.1),
        Pattern::Attribute { attr, pattern } => {
            collect_attribute_errors(&attr.0, source, path, buckets);
            collect_pattern_errors(pattern, source, path, buckets);
        }
        Pattern::Typed(inner, ty) | Pattern::AsType(inner, ty) => {
            collect_pattern_errors(inner, source, path, buckets);
            collect_type_expr_errors(ty, source, path, buckets);
        }
        Pattern::Tuple(items) | Pattern::List(items) | Pattern::Vector(items) => {
            for item in items {
                collect_pattern_errors(item, source, path, buckets);
            }
        }
        Pattern::App { args, .. } => {
            for arg in args {
                collect_pattern_errors(arg, source, path, buckets);
            }
        }
        Pattern::Index { index, .. } => collect_type_expr_errors(index, source, path, buckets),
        Pattern::RangeIndex { start, end, .. } => {
            collect_type_expr_errors(start, source, path, buckets);
            collect_type_expr_errors(end, source, path, buckets);
        }
        Pattern::Struct { fields, .. } => {
            for field in fields {
                collect_field_pattern_errors(&field.0, source, path, buckets);
            }
        }
        Pattern::Infix { lhs, rhs, .. } => {
            collect_pattern_errors(lhs, source, path, buckets);
            collect_pattern_errors(rhs, source, path, buckets);
        }
        Pattern::AsBinding { pattern, .. } => {
            collect_pattern_errors(pattern, source, path, buckets);
        }
        Pattern::Wild | Pattern::Literal(_) | Pattern::Ident(_) | Pattern::TypeVar(_) => {}
    }
}

fn collect_field_pattern_errors(
    field: &FieldPattern,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match field {
        FieldPattern::Binding { pattern, .. } => {
            collect_pattern_errors(pattern, source, path, buckets)
        }
        FieldPattern::Shorthand(_) | FieldPattern::Wild(_) => {}
    }
}

fn collect_let_binding_errors(
    binding: &LetBinding,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    collect_pattern_errors(&binding.pattern, source, path, buckets);
    collect_expr_errors(&binding.value, source, path, buckets);
}

fn collect_match_case_errors(
    case: &MatchCase,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    for attr in &case.attrs {
        collect_attribute_errors(&attr.0, source, path, buckets);
    }
    collect_pattern_errors(&case.pattern, source, path, buckets);
    if let Some(guard) = &case.guard {
        collect_expr_errors(guard, source, path, buckets);
    }
    collect_expr_errors(&case.body, source, path, buckets);
}

fn collect_field_expr_errors(
    field: &FieldExpr,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match field {
        FieldExpr::Assignment { target, value } => {
            collect_expr_errors(target, source, path, buckets);
            collect_expr_errors(value, source, path, buckets);
        }
        FieldExpr::Shorthand(_) => {}
    }
}

fn collect_vector_update_errors(
    update: &VectorUpdate,
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match update {
        VectorUpdate::Assignment { target, value } => {
            collect_expr_errors(target, source, path, buckets);
            collect_expr_errors(value, source, path, buckets);
        }
        VectorUpdate::RangeAssignment { start, end, value } => {
            collect_expr_errors(start, source, path, buckets);
            collect_expr_errors(end, source, path, buckets);
            collect_expr_errors(value, source, path, buckets);
        }
        VectorUpdate::Shorthand(_) => {}
    }
}

fn collect_expr_errors(
    expr: &(Expr, Span),
    source: &str,
    path: &Path,
    buckets: &mut BTreeMap<(ErrorKind, String), SampleBucket>,
) {
    match &expr.0 {
        Expr::Error(_) => record_error(buckets, ErrorKind::Expr, source, path, expr.1),
        Expr::Attribute { attr, expr } => {
            collect_attribute_errors(&attr.0, source, path, buckets);
            collect_expr_errors(expr, source, path, buckets);
        }
        Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
            collect_expr_errors(lhs, source, path, buckets);
            collect_expr_errors(rhs, source, path, buckets);
        }
        Expr::Let { binding, body } => {
            collect_let_binding_errors(binding, source, path, buckets);
            collect_expr_errors(body, source, path, buckets);
        }
        Expr::Var {
            target,
            value,
            body,
        } => {
            collect_expr_errors(target, source, path, buckets);
            collect_expr_errors(value, source, path, buckets);
            collect_expr_errors(body, source, path, buckets);
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Let(binding) => {
                        collect_let_binding_errors(binding, source, path, buckets)
                    }
                    BlockItem::Var { target, value } => {
                        collect_expr_errors(target, source, path, buckets);
                        collect_expr_errors(value, source, path, buckets);
                    }
                    BlockItem::Expr(expr) => collect_expr_errors(expr, source, path, buckets),
                }
            }
        }
        Expr::Return(expr)
        | Expr::Throw(expr)
        | Expr::Exit(Some(expr))
        | Expr::Prefix { expr, .. }
        | Expr::Field { expr, .. } => collect_expr_errors(expr, source, path, buckets),
        Expr::Assert { condition, message } => {
            collect_expr_errors(condition, source, path, buckets);
            if let Some(message) = message {
                collect_expr_errors(message, source, path, buckets);
            }
        }
        Expr::Exit(None) => {}
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_errors(cond, source, path, buckets);
            collect_expr_errors(then_branch, source, path, buckets);
            if let Some(else_branch) = else_branch {
                collect_expr_errors(else_branch, source, path, buckets);
            }
        }
        Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
            collect_expr_errors(scrutinee, source, path, buckets);
            for case in cases {
                collect_match_case_errors(&case.0, source, path, buckets);
            }
        }
        Expr::Foreach(foreach) => {
            collect_expr_errors(&foreach.start, source, path, buckets);
            collect_expr_errors(&foreach.end, source, path, buckets);
            if let Some(step) = &foreach.step {
                collect_expr_errors(step, source, path, buckets);
            }
            if let Some(ty) = &foreach.ty {
                collect_type_expr_errors(ty, source, path, buckets);
            }
            collect_expr_errors(&foreach.body, source, path, buckets);
        }
        Expr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                collect_expr_errors(measure, source, path, buckets);
            }
            collect_expr_errors(body, source, path, buckets);
            collect_expr_errors(until, source, path, buckets);
        }
        Expr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                collect_expr_errors(measure, source, path, buckets);
            }
            collect_expr_errors(cond, source, path, buckets);
            collect_expr_errors(body, source, path, buckets);
        }
        Expr::Cast { expr, ty } => {
            collect_expr_errors(expr, source, path, buckets);
            collect_type_expr_errors(ty, source, path, buckets);
        }
        Expr::Call(call) => {
            collect_expr_errors(&call.callee, source, path, buckets);
            for arg in &call.args {
                collect_expr_errors(arg, source, path, buckets);
            }
        }
        Expr::SizeOf(ty) | Expr::Constraint(ty) => {
            collect_type_expr_errors(ty, source, path, buckets);
        }
        Expr::Index { expr, index } => {
            collect_expr_errors(expr, source, path, buckets);
            collect_expr_errors(index, source, path, buckets);
        }
        Expr::Slice { expr, start, end } => {
            collect_expr_errors(expr, source, path, buckets);
            collect_expr_errors(start, source, path, buckets);
            collect_expr_errors(end, source, path, buckets);
        }
        Expr::VectorSlice {
            expr,
            start,
            length,
        } => {
            collect_expr_errors(expr, source, path, buckets);
            collect_expr_errors(start, source, path, buckets);
            collect_expr_errors(length, source, path, buckets);
        }
        Expr::Struct { fields, .. } => {
            for field in fields {
                collect_field_expr_errors(&field.0, source, path, buckets);
            }
        }
        Expr::Update { base, fields } => {
            collect_expr_errors(base, source, path, buckets);
            for field in fields {
                collect_field_expr_errors(&field.0, source, path, buckets);
            }
        }
        Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
            for item in items {
                collect_expr_errors(item, source, path, buckets);
            }
        }
        Expr::VectorUpdate { base, updates } => {
            collect_expr_errors(base, source, path, buckets);
            for update in updates {
                collect_vector_update_errors(&update.0, source, path, buckets);
            }
        }
        Expr::Literal(_) | Expr::Ident(_) | Expr::TypeVar(_) | Expr::Ref(_) | Expr::Config(_) => {}
    }
}
