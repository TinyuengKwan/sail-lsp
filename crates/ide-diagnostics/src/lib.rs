//! Diagnostic computation for Sail source files.
//!
//! Raw diagnostics from `hir-ty` are "cooked" into `AnyDiagnostic` variants,
//! then dispatched to individual handler modules. Each handler converts its
//! variant into an IDE `Diagnostic` with optional quick-fixes.
//!
//! Two pipelines: `syntax_diagnostics` (parse errors, bracket checks) and
//! `semantic_diagnostics` (type inference, name resolution, lints).
//! Combined via `full_diagnostics`.

mod handlers {
    pub(crate) mod circular_include;
    pub(crate) mod concat_type_mismatch;
    pub(crate) mod deprecated_syntax;
    pub(crate) mod duplicate_binding;
    pub(crate) mod duplicate_definition;
    pub(crate) mod effect_mismatch_in_override;
    pub(crate) mod effect_violation;
    pub(crate) mod expected_function;
    pub(crate) mod impossible_constraint;
    pub(crate) mod incomplete_scattered;
    pub(crate) mod inconsistent_hex_casing;
    pub(crate) mod invalid_list_pattern;
    pub(crate) mod invalid_slice_assign;
    pub(crate) mod invalid_string_pattern;
    pub(crate) mod invalid_vector_concat;
    pub(crate) mod mapping_binding_mismatch;
    pub(crate) mod mismatched_arg_count;
    pub(crate) mod missing_effect_annotation;
    pub(crate) mod missing_fields;
    pub(crate) mod missing_match_arms;
    pub(crate) mod no_overloading;
    pub(crate) mod non_contiguous_subrange;
    pub(crate) mod redundant_type_annotation;
    pub(crate) mod remove_trailing_return;
    pub(crate) mod remove_unnecessary_else;
    pub(crate) mod return_outside_function;
    pub(crate) mod shadowed_binding;
    pub(crate) mod type_mismatch;
    pub(crate) mod undeclared_effect;
    pub(crate) mod undeclared_mapping_type;
    pub(crate) mod unnecessary_mutability;
    pub(crate) mod unreachable_code;
    pub(crate) mod unresolved_field;
    pub(crate) mod unresolved_ident;
    pub(crate) mod unresolved_include;
    pub(crate) mod unresolved_quants;
    pub(crate) mod unsolved_constraint;
    pub(crate) mod unused_import;
    pub(crate) mod unused_variables;
    pub(crate) mod vector_subrange_order;
}
pub mod message;
pub mod parse;
pub mod reporting;
pub mod semantic;
#[cfg(test)]
pub(crate) mod tests;
pub mod type_error;
mod types;

use hir::diagnostics::{
    AnyDiagnostic, CircularInclude, ConcatTypeMismatch, DeprecatedSyntax, DuplicateBindingDiag,
    DuplicateDefinition, EffectMismatchInOverride, EffectViolation, ExpectedFunction,
    ImpossibleConstraint, IncompleteMatch, IncompleteScattered, InconsistentHexCasing,
    InvalidListPattern, InvalidSliceAssign, InvalidStringPattern, InvalidVectorConcat,
    MappingBindingMismatch, MismatchedArgCount, MissingEffectAnnotation, MissingFields,
    NoOverloading, NonContiguousSubrange, RedundantTypeAnnotation, RemoveTrailingReturn,
    RemoveUnnecessaryElse, ReturnOutsideFunction, ShadowedBinding, TypeMismatch, UndeclaredEffect,
    UndeclaredMappingType, UnnecessaryMutability, UnreachableCode, UnresolvedField,
    UnresolvedIdent, UnresolvedInclude, UnresolvedQuants, UnsolvedConstraint, UnusedImport,
    UnusedVariable, VectorSubrangeOrder,
};
use ide_db::assists::{Assist, AssistId, AssistKind, AssistResolveStrategy, Label, SnippetCap};
use ide_db::line_index::TextRange;
use ide_db::source_change::SourceChange;

/// Extract a `FileRange` from an `InFile<SyntaxNodePtr>`.
///
/// Convenience helper so handlers can write `node_file_range(&d.node)` instead
/// of constructing `FileRange { file_id: d.node.file_id, range: d.node.value.text_range() }`.
pub(crate) fn node_file_range(
    node: &hir_def::in_file::InFile<syntax::SyntaxNodePtr>,
) -> hir_def::in_file::FileRange {
    hir_def::in_file::FileRange { file_id: node.file_id, range: node.value.text_range() }
}

// Re-export Severity from hir-def (single source of truth).
pub use hir_def::diagnostics::Severity;

/// Configuration for diagnostics computation.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsConfig {
    /// Whether diagnostics are enabled at all.
    pub enabled: bool,
    /// Diagnostic codes to disable (e.g., "unused-variable").
    pub disabled: std::collections::HashSet<String>,
    /// Whether to suppress experimental diagnostics.
    pub disable_experimental: bool,
    /// Diagnostic codes to show as hints instead of warnings.
    pub warnings_as_hint: std::collections::HashSet<String>,
    /// Diagnostic codes to show as info instead of warnings.
    pub warnings_as_info: std::collections::HashSet<String>,
    /// Path prefix remapping for diagnostics (e.g., to strip workspace root).
    pub remap_prefix: std::collections::HashMap<String, String>,
    /// Enable style/lint diagnostics.
    pub style_lints: bool,
    /// Maximum number of diagnostics to report per file.
    /// `None` means unlimited.
    pub max_diagnostics_per_file: Option<usize>,
    /// Whether the client supports snippet text edits in fixes.
    pub snippet_cap: Option<SnippetCap>,
}

impl DiagnosticsConfig {
    pub fn new() -> Self {
        // Default-disable noisy warnings. Users can re-enable via LSP config.
        let mut disabled = std::collections::HashSet::new();
        disabled.insert("deprecated-effect-annotation".to_owned());
        disabled.insert("missing-extern-purity".to_owned());
        Self {
            enabled: true,
            disabled,
            disable_experimental: false,
            warnings_as_hint: std::collections::HashSet::new(),
            warnings_as_info: std::collections::HashSet::new(),
            remap_prefix: std::collections::HashMap::new(),
            style_lints: true,
            max_diagnostics_per_file: Some(128),
            snippet_cap: None,
        }
    }

    /// Check whether a diagnostic code is enabled.
    pub fn is_enabled(&self, code: &str) -> bool {
        self.enabled && !self.disabled.contains(code)
    }

    /// Return the effective severity for a diagnostic code.
    pub fn effective_severity(
        &self,
        code: &str,
        default: hir_def::diagnostics::Severity,
    ) -> hir_def::diagnostics::Severity {
        use hir_def::diagnostics::Severity;
        if self.warnings_as_hint.contains(code) {
            Severity::WeakWarning
        } else if self.warnings_as_info.contains(code) {
            Severity::Information
        } else {
            default
        }
    }
}

/// A diagnostic with byte-offset range.
///
/// Replaces `lsp_types::Diagnostic` in IDE crates.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: hir_def::diagnostics::DiagnosticCode,
    pub message: String,
    pub range: hir_def::in_file::FileRange,
    pub severity: Severity,
    pub unused: bool,
    pub experimental: bool,
    /// Fix actions attached to this diagnostic.
    pub fixes: Option<Vec<Assist>>,
    /// The syntax node this diagnostic is attached to.
    ///
    /// Used for `#[allow]` attribute resolution in RA; in Sail, used
    /// for precise range adjustment in handlers.
    pub main_node: Option<hir_def::in_file::InFile<syntax::SyntaxNodePtr>>,
}

impl Diagnostic {
    /// Create a new diagnostic.
    pub fn new(
        code: hir_def::diagnostics::DiagnosticCode,
        message: impl Into<String>,
        range: impl Into<hir_def::in_file::FileRange>,
    ) -> Self {
        let severity = code.default_severity();
        Self {
            code,
            message: message.into(),
            range: range.into(),
            severity,
            unused: false,
            experimental: false,
            fixes: None,
            main_node: None,
        }
    }

    /// Create a diagnostic from a syntax node pointer, computing the display range.
    pub(crate) fn new_with_syntax_node_ptr(
        ctx: &DiagnosticsContext<'_>,
        code: hir_def::diagnostics::DiagnosticCode,
        message: impl Into<String>,
        node: hir_def::in_file::InFile<syntax::SyntaxNodePtr>,
    ) -> Self {
        let file_range = ctx.sema.diagnostics_display_range(node);
        Diagnostic::new(
            code,
            message,
            hir_def::in_file::FileRange { file_id: node.file_id, range: file_range.range },
        )
        .with_main_node(node)
    }

    /// Set severity override.
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Mark as unused (adds Unnecessary tag).
    pub fn with_unused(mut self, unused: bool) -> Self {
        self.unused = unused;
        self
    }

    /// Attach fix actions.
    pub fn with_fixes(mut self, fixes: Option<Vec<Assist>>) -> Self {
        self.fixes = fixes;
        self
    }

    /// Builder: mark as stable (non-experimental).
    pub fn stable(mut self) -> Self {
        self.experimental = false;
        self
    }

    /// Builder: mark as experimental.
    pub fn experimental(mut self) -> Self {
        self.experimental = true;
        self
    }

    /// Builder: set main syntax node for attribute resolution.
    pub fn with_main_node(
        mut self,
        main_node: hir_def::in_file::InFile<syntax::SyntaxNodePtr>,
    ) -> Self {
        self.main_node = Some(main_node);
        self
    }
}

/// Context for computing diagnostics for a file.
///
/// Private struct -- visible to handlers via Rust module hierarchy.
struct DiagnosticsContext<'a> {
    config: &'a DiagnosticsConfig,
    sema: hir::Semantics<'a>,
    /// Controls lazy resolution of fix actions.
    #[allow(dead_code)]
    resolve: &'a AssistResolveStrategy,
}

impl<'a> DiagnosticsContext<'a> {
    #[allow(dead_code)]
    fn db(&self) -> &'a dyn salsa::Database {
        self.sema.db
    }
}

/// Cross-file name sets for suppressing false-positive diagnostics.
///
/// Per-file inference lacks cross-file data, so we pass workspace-level
/// name sets to filter diagnostics that reference symbols defined in
/// other files. Built by `Analysis::file_diagnostics` from all workspace
/// files' `top_level_env` results.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceNames {
    /// Function names defined across all workspace files.
    pub function_names: std::collections::HashSet<String>,
    /// Constructor names (enum/union members) across all workspace files.
    pub constructor_names: std::collections::HashSet<String>,
    /// Value names (let bindings, registers) across all workspace files.
    pub value_names: std::collections::HashSet<String>,
    /// Record/struct names that have known field definitions elsewhere.
    pub record_names: std::collections::HashSet<String>,
}

// Two-pipeline entry points.

/// Request both syntax and semantic diagnostics for a file.
pub fn full_diagnostics(
    db: &dyn salsa::Database,
    config: &DiagnosticsConfig,
    resolve: &AssistResolveStrategy,
    file: &dyn ide_db::FileDb,
    ft: base_db::FileText,
    workspace_names: Option<&WorkspaceNames>,
) -> Vec<Diagnostic> {
    let mut res = syntax_diagnostics(db, config, file, ft);
    res.extend(semantic_diagnostics(db, config, resolve, file, ft, workspace_names));
    res
}

/// Request parser-level diagnostics for a file.
pub fn syntax_diagnostics(
    db: &dyn salsa::Database,
    config: &DiagnosticsConfig,
    file: &dyn ide_db::FileDb,
    ft: base_db::FileText,
) -> Vec<Diagnostic> {
    if config.disabled.contains("syntax-error") {
        return Vec::new();
    }

    let mut res = Vec::new();

    // Phase 1: Parse errors via firewall query.
    // Uses `parse_errors()` firewall query so downstream doesn't re-run
    // when parse changed but errors didn't.
    let errors = syntax::parse_query::parse_errors(db, ft);
    if let Some(errors) = errors {
        for err in errors.iter().take(128) {
            res.push(Diagnostic::new(
                hir_def::diagnostics::DiagnosticCode::SyntaxError,
                format!("Syntax Error: {err}"),
                err.range(),
            ));
        }
    }

    // Phase 2: Bracket matching + ItemTree checks (Sail-specific).
    let parse_diags = compute_parse_diagnostics(file, &[]);
    res.extend(parse_diags.into_iter().take(128).map(|d| hir_diag_to_ide(&d)));

    // Config-based filtering for syntax pipeline.
    res.retain(|d| {
        !(config.disabled.contains(d.code.as_str())
            || config.disable_experimental && d.experimental)
    });

    res
}

/// Request semantic diagnostics for a file.
///
/// Combines:
/// 1. AST visitor checks (duplicate definitions, unreachable code, etc.)
/// 2. Per-callable inference diagnostics via salsa queries
/// 3. Cross-file false-positive suppression via WorkspaceNames
/// 4. Config-based filtering
pub fn semantic_diagnostics(
    db: &dyn salsa::Database,
    config: &DiagnosticsConfig,
    resolve: &AssistResolveStrategy,
    file: &dyn ide_db::FileDb,
    ft: base_db::FileText,
    workspace_names: Option<&WorkspaceNames>,
) -> Vec<Diagnostic> {
    let sema = hir::Semantics::new(db);
    let ctx = DiagnosticsContext { config, sema, resolve };
    let mut res = Vec::new();

    // AST visitor checks: duplicate_definitions, unreachable_code.
    let semantic_diags = compute_semantic_diagnostics(file);
    for d in &semantic_diags {
        res.push(hir_diag_to_ide(d));
    }

    // Per-callable inference diagnostics.
    let callable_ids = hir_def::def_query::file_def_with_body_ids(db, ft);
    let mut any_diags: Vec<AnyDiagnostic> = Vec::new();

    for &id in callable_ids {
        let tcr = hir_ty::query::infer(db, id);

        // InferenceDiagnostic -> AnyDiagnostic
        let typed_diags = tcr.0.inference_diagnostics();
        let has_typed_diags = !typed_diags.is_empty();
        let has_type_mismatches = !tcr.0.type_mismatches.is_empty();

        if has_typed_diags || has_type_mismatches {
            let bsm = hir_def::def_query::body_with_source_map(db, id);
            let source_map = &bsm.0 .1;

            for diag in typed_diags {
                if let Some(d) = AnyDiagnostic::inference_diagnostic(diag, source_map) {
                    any_diags.push(d);
                }
            }

            // 2c. TypeMismatch cooking.
            any_diags
                .extend(AnyDiagnostic::from_type_mismatches(&tcr.0.type_mismatches, source_map));
        }
    }

    // Cross-file false-positive suppression: when `workspace_names` is
    // provided, suppress diagnostics for identifiers defined in other files.
    if let Some(ws) = workspace_names {
        any_diags.retain(|d| !is_cross_file_false_positive(d, ws));
    }

    // Dispatch each AnyDiagnostic to its handler.
    for diag in &any_diags {
        #[rustfmt::skip]
        let d = match diag {
            AnyDiagnostic::UnresolvedIdent(d) =>
                handlers::unresolved_ident::unresolved_ident(&ctx, d),
            AnyDiagnostic::UnresolvedField(d) =>
                handlers::unresolved_field::unresolved_field(&ctx, d),
            AnyDiagnostic::MismatchedArgCount(d) =>
                handlers::mismatched_arg_count::mismatched_arg_count(&ctx, d),
            AnyDiagnostic::ExpectedFunction(d) =>
                handlers::expected_function::expected_function(&ctx, d),
            AnyDiagnostic::EffectViolation(d) =>
                handlers::effect_violation::effect_violation(&ctx, d),
            AnyDiagnostic::IncompleteMatch(d) =>
                handlers::missing_match_arms::missing_match_arms(&ctx, d),
            AnyDiagnostic::MissingFields(d) =>
                handlers::missing_fields::missing_fields(&ctx, d),
            AnyDiagnostic::UnsolvedConstraint(d) =>
                handlers::unsolved_constraint::unsolved_constraint(&ctx, d),
            AnyDiagnostic::TypeMismatch(d) => match handlers::type_mismatch::type_mismatch(&ctx, d) {
                Some(it) => it,
                None => continue,
            },
            AnyDiagnostic::UnusedVariable(d) =>
                handlers::unused_variables::unused_variables(&ctx, d),
            AnyDiagnostic::RemoveTrailingReturn(d) =>
                handlers::remove_trailing_return::remove_trailing_return(&ctx, d),
            AnyDiagnostic::RemoveUnnecessaryElse(d) =>
                handlers::remove_unnecessary_else::remove_unnecessary_else(&ctx, d),
            AnyDiagnostic::ConcatTypeMismatch(d) =>
                handlers::concat_type_mismatch::concat_type_mismatch(&ctx, d),
            AnyDiagnostic::NoOverloading(d) =>
                handlers::no_overloading::no_overloading(&ctx, d),
            AnyDiagnostic::DuplicateBinding(d) =>
                handlers::duplicate_binding::duplicate_binding(&ctx, d),
            AnyDiagnostic::NonContiguousSubrange(d) =>
                handlers::non_contiguous_subrange::non_contiguous_subrange(&ctx, d),
            AnyDiagnostic::VectorSubrangeOrder(d) =>
                handlers::vector_subrange_order::vector_subrange_order(&ctx, d),
            AnyDiagnostic::MappingBindingMismatch(d) =>
                handlers::mapping_binding_mismatch::mapping_binding_mismatch(&ctx, d),
            AnyDiagnostic::UnresolvedInclude(d) =>
                handlers::unresolved_include::unresolved_include(&ctx, d),
            AnyDiagnostic::DuplicateDefinition(d) =>
                handlers::duplicate_definition::duplicate_definition(&ctx, d),
            AnyDiagnostic::IncompleteScattered(d) =>
                handlers::incomplete_scattered::incomplete_scattered(&ctx, d),
            AnyDiagnostic::CircularInclude(d) =>
                handlers::circular_include::circular_include(&ctx, d),
            AnyDiagnostic::ShadowedBinding(d) =>
                handlers::shadowed_binding::shadowed_binding(&ctx, d),
            AnyDiagnostic::ReturnOutsideFunction(d) =>
                handlers::return_outside_function::return_outside_function(&ctx, d),
            AnyDiagnostic::InvalidVectorConcat(d) =>
                handlers::invalid_vector_concat::invalid_vector_concat(&ctx, d),
            AnyDiagnostic::InvalidListPattern(d) =>
                handlers::invalid_list_pattern::invalid_list_pattern(&ctx, d),
            AnyDiagnostic::InvalidStringPattern(d) =>
                handlers::invalid_string_pattern::invalid_string_pattern(&ctx, d),
            AnyDiagnostic::ImpossibleConstraint(d) =>
                handlers::impossible_constraint::impossible_constraint(&ctx, d),
            AnyDiagnostic::UnresolvedQuants(d) =>
                handlers::unresolved_quants::unresolved_quants(&ctx, d),
            AnyDiagnostic::InvalidSliceAssign(d) =>
                handlers::invalid_slice_assign::invalid_slice_assign(&ctx, d),
            AnyDiagnostic::UndeclaredMappingType(d) =>
                handlers::undeclared_mapping_type::undeclared_mapping_type(&ctx, d),
            AnyDiagnostic::UnusedImport(d) =>
                handlers::unused_import::unused_import(&ctx, d),
            AnyDiagnostic::UnnecessaryMutability(d) =>
                handlers::unnecessary_mutability::unnecessary_mutability(&ctx, d),
            AnyDiagnostic::DeprecatedSyntax(d) =>
                handlers::deprecated_syntax::deprecated_syntax(&ctx, d),
            AnyDiagnostic::InconsistentHexCasing(d) =>
                handlers::inconsistent_hex_casing::inconsistent_hex_casing(&ctx, d),
            AnyDiagnostic::UnreachableCode(d) =>
                handlers::unreachable_code::unreachable_code(&ctx, d),
            AnyDiagnostic::RedundantTypeAnnotation(d) =>
                handlers::redundant_type_annotation::redundant_type_annotation(&ctx, d),
            AnyDiagnostic::MissingEffectAnnotation(d) =>
                handlers::missing_effect_annotation::missing_effect_annotation(&ctx, d),
            AnyDiagnostic::UndeclaredEffect(d) =>
                handlers::undeclared_effect::undeclared_effect(&ctx, d),
            AnyDiagnostic::EffectMismatchInOverride(d) =>
                handlers::effect_mismatch_in_override::effect_mismatch_in_override(&ctx, d),
        };
        res.push(d);
    }

    // Config-based filtering.
    res.retain(|d| {
        !(ctx.config.disabled.contains(d.code.as_str())
            || ctx.config.disable_experimental && d.experimental)
    });

    // Apply config-level severity overrides.
    handle_lints(ctx.config, &mut res);

    res
}

/// Check if an `AnyDiagnostic` is a false positive caused by
/// cross-file resolution failure.
///
/// Returns `true` if the diagnostic should be suppressed because the
/// referenced symbol exists in another workspace file.
fn is_cross_file_false_positive(d: &AnyDiagnostic, ws: &WorkspaceNames) -> bool {
    match d {
        AnyDiagnostic::UnresolvedIdent(d) => {
            ws.function_names.contains(&d.name)
                || ws.constructor_names.contains(&d.name)
                || ws.value_names.contains(&d.name)
        }
        AnyDiagnostic::UnresolvedField(d) => {
            // If the receiver type is a record defined in another file,
            // field resolution may fail per-file but succeed workspace-wide.
            // Query type name directly from Ty instead of parsing strings.
            d.receiver.as_name().map(|n| ws.record_names.contains(n)).unwrap_or(false)
        }
        AnyDiagnostic::MismatchedArgCount(d) => {
            // Cross-file function calls may have arity mismatch due to
            // missing overload information. Suppress if the function
            // exists in the workspace.
            // NOTE: We don't have the function name in MismatchedArgCount.
            // This variant is kept as-is; false positives here are rare
            // because MismatchedArgCount requires the function to be found.
            let _ = d;
            false
        }
        _ => false,
    }
}

/// Convert `hir_def::diagnostics::Diagnostic` to `ide_diagnostics::Diagnostic`.
fn hir_diag_to_ide(d: &hir_def::diagnostics::Diagnostic) -> Diagnostic {
    let unused =
        d.tags.iter().any(|t| matches!(t, hir_def::diagnostics::DiagnosticTag::Unnecessary));
    Diagnostic::new(d.code.clone(), d.message.clone(), d.range)
        .with_severity(d.severity)
        .with_unused(unused)
}

// Kept for unit tests that need to dispatch a single AnyDiagnostic.

#[cfg(test)]
pub(crate) fn dispatch_one(ctx: &DiagnosticsContext<'_>, d: &AnyDiagnostic) -> Option<Diagnostic> {
    Some(match d {
        AnyDiagnostic::UnresolvedIdent(d) => handlers::unresolved_ident::unresolved_ident(ctx, d),
        AnyDiagnostic::UnresolvedField(d) => handlers::unresolved_field::unresolved_field(ctx, d),
        AnyDiagnostic::MismatchedArgCount(d) => {
            handlers::mismatched_arg_count::mismatched_arg_count(ctx, d)
        }
        AnyDiagnostic::ExpectedFunction(d) => {
            handlers::expected_function::expected_function(ctx, d)
        }
        AnyDiagnostic::EffectViolation(d) => handlers::effect_violation::effect_violation(ctx, d),
        AnyDiagnostic::IncompleteMatch(d) => {
            handlers::missing_match_arms::missing_match_arms(ctx, d)
        }
        AnyDiagnostic::MissingFields(d) => handlers::missing_fields::missing_fields(ctx, d),
        AnyDiagnostic::UnsolvedConstraint(d) => {
            handlers::unsolved_constraint::unsolved_constraint(ctx, d)
        }
        AnyDiagnostic::TypeMismatch(d) => handlers::type_mismatch::type_mismatch(ctx, d)?,
        AnyDiagnostic::UnusedVariable(d) => handlers::unused_variables::unused_variables(ctx, d),
        AnyDiagnostic::RemoveTrailingReturn(d) => {
            handlers::remove_trailing_return::remove_trailing_return(ctx, d)
        }
        AnyDiagnostic::RemoveUnnecessaryElse(d) => {
            handlers::remove_unnecessary_else::remove_unnecessary_else(ctx, d)
        }
        AnyDiagnostic::ConcatTypeMismatch(d) => {
            handlers::concat_type_mismatch::concat_type_mismatch(ctx, d)
        }
        AnyDiagnostic::NoOverloading(d) => handlers::no_overloading::no_overloading(ctx, d),
        AnyDiagnostic::DuplicateBinding(d) => {
            handlers::duplicate_binding::duplicate_binding(ctx, d)
        }
        AnyDiagnostic::NonContiguousSubrange(d) => {
            handlers::non_contiguous_subrange::non_contiguous_subrange(ctx, d)
        }
        AnyDiagnostic::VectorSubrangeOrder(d) => {
            handlers::vector_subrange_order::vector_subrange_order(ctx, d)
        }
        AnyDiagnostic::MappingBindingMismatch(d) => {
            handlers::mapping_binding_mismatch::mapping_binding_mismatch(ctx, d)
        }
        AnyDiagnostic::UnresolvedInclude(d) => {
            handlers::unresolved_include::unresolved_include(ctx, d)
        }
        AnyDiagnostic::DuplicateDefinition(d) => {
            handlers::duplicate_definition::duplicate_definition(ctx, d)
        }
        AnyDiagnostic::IncompleteScattered(d) => {
            handlers::incomplete_scattered::incomplete_scattered(ctx, d)
        }
        AnyDiagnostic::CircularInclude(d) => handlers::circular_include::circular_include(ctx, d),
        AnyDiagnostic::ShadowedBinding(d) => handlers::shadowed_binding::shadowed_binding(ctx, d),
        AnyDiagnostic::ReturnOutsideFunction(d) => {
            handlers::return_outside_function::return_outside_function(ctx, d)
        }
        AnyDiagnostic::InvalidVectorConcat(d) => {
            handlers::invalid_vector_concat::invalid_vector_concat(ctx, d)
        }
        AnyDiagnostic::InvalidListPattern(d) => {
            handlers::invalid_list_pattern::invalid_list_pattern(ctx, d)
        }
        AnyDiagnostic::InvalidStringPattern(d) => {
            handlers::invalid_string_pattern::invalid_string_pattern(ctx, d)
        }
        AnyDiagnostic::ImpossibleConstraint(d) => {
            handlers::impossible_constraint::impossible_constraint(ctx, d)
        }
        AnyDiagnostic::UnresolvedQuants(d) => {
            handlers::unresolved_quants::unresolved_quants(ctx, d)
        }
        AnyDiagnostic::InvalidSliceAssign(d) => {
            handlers::invalid_slice_assign::invalid_slice_assign(ctx, d)
        }
        AnyDiagnostic::UndeclaredMappingType(d) => {
            handlers::undeclared_mapping_type::undeclared_mapping_type(ctx, d)
        }
        AnyDiagnostic::UnusedImport(d) => handlers::unused_import::unused_import(ctx, d),
        AnyDiagnostic::UnnecessaryMutability(d) => {
            handlers::unnecessary_mutability::unnecessary_mutability(ctx, d)
        }
        AnyDiagnostic::DeprecatedSyntax(d) => {
            handlers::deprecated_syntax::deprecated_syntax(ctx, d)
        }
        AnyDiagnostic::InconsistentHexCasing(d) => {
            handlers::inconsistent_hex_casing::inconsistent_hex_casing(ctx, d)
        }
        AnyDiagnostic::UnreachableCode(d) => handlers::unreachable_code::unreachable_code(ctx, d),
        AnyDiagnostic::RedundantTypeAnnotation(d) => {
            handlers::redundant_type_annotation::redundant_type_annotation(ctx, d)
        }
        AnyDiagnostic::MissingEffectAnnotation(d) => {
            handlers::missing_effect_annotation::missing_effect_annotation(ctx, d)
        }
        AnyDiagnostic::UndeclaredEffect(d) => {
            handlers::undeclared_effect::undeclared_effect(ctx, d)
        }
        AnyDiagnostic::EffectMismatchInOverride(d) => {
            handlers::effect_mismatch_in_override::effect_mismatch_in_override(ctx, d)
        }
    })
}

/// Apply config-level severity overrides to diagnostics.
///
/// Currently applies `DiagnosticsConfig` overrides (`warnings_as_hint`,
/// `warnings_as_info`). When Sail gains `@lint` directive support,
/// attribute resolution logic will be added here.
fn handle_lints(config: &DiagnosticsConfig, diagnostics: &mut Vec<Diagnostic>) {
    for diag in diagnostics.iter_mut() {
        let code = diag.code.as_str();
        // Config-level severity override:
        //   warnings_as_hint → WeakWarning
        //   warnings_as_info → Information
        if config.warnings_as_hint.contains(code) {
            diag.severity = hir_def::diagnostics::Severity::WeakWarning;
        } else if config.warnings_as_info.contains(code) {
            diag.severity = hir_def::diagnostics::Severity::Information;
        }
    }

    // Filter out Allow-severity diagnostics.
    diagnostics.retain(|d| {
        d.severity != hir_def::diagnostics::Severity::Hint || {
            // Keep hints that are explicitly enabled.
            true
        }
    });
}

/// Narrow a diagnostic's display range to a salient token.
///
/// The adjustment function extracts sub-ranges from complex expressions
/// (e.g., the `if` keyword from an IfExpr, or the `}` from a BlockExpr)
/// to make diagnostics point at the most relevant token.
///
/// When `Semantics::parse(file_id)` is available, this function will
/// resolve the `AstPtr` to a concrete node and call `adj` on it.
/// For now, falls back to the full pointer range.
#[allow(dead_code)]
fn adjusted_display_range<N: syntax::ast::AstNode>(
    _ctx: &DiagnosticsContext<'_>,
    diag_ptr: hir_def::in_file::InFile<syntax::AstPtr<N>>,
    _adj: &dyn Fn(N) -> Option<base_db::TextRange>,
) -> hir_def::in_file::FileRange {
    // TODO: resolve AstPtr via ctx.sema.parse(file_id) when available.
    // For now, use the full pointer range as fallback.
    hir_def::in_file::FileRange { file_id: diag_ptr.file_id, range: diag_ptr.value.text_range() }
}

// Minimal re-exports for public API.

pub use parse::compute_parse_diagnostics;
pub use semantic::compute_semantic_diagnostics;

/// Create an `Assist` (fix) with a source change.
pub fn fix(
    id: &'static str,
    label: &str,
    source_change: SourceChange,
    target: TextRange,
) -> Assist {
    let mut res = unresolved_fix(id, label, target);
    res.source_change = Some(source_change);
    res
}

/// Create an `Assist` stub without a source change (for lazy resolution).
pub fn unresolved_fix(id: &'static str, label: &str, target: TextRange) -> Assist {
    assert!(!id.contains(' '));
    Assist {
        id: AssistId(id, AssistKind::QuickFix),
        label: Label::new(label.to_owned()),
        group: None,
        target,
        source_change: None,
        command: None,
        edits: Vec::new(),
    }
}

// Tests moved to tests.rs.
