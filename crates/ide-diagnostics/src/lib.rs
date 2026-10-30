//! Diagnostic computation for Sail source files.
//!
//! Raw diagnostics from `hir-ty` are "cooked" into `AnyDiagnostic` variants,
//! then dispatched to individual handler modules. Each handler converts its
//! variant into an IDE `Diagnostic` with optional quick-fixes.
//!
//! Two pipelines: `syntax_diagnostics` (parse + tree validation errors) and
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
    pub(crate) mod mismatched_param_count;
    pub(crate) mod missing_effect_annotation;
    pub(crate) mod missing_fields;
    pub(crate) mod missing_match_arms;
    pub(crate) mod no_overloading;
    pub(crate) mod non_contiguous_subrange;
    pub(crate) mod not_a_function;
    pub(crate) mod redundant_match_arm;
    pub(crate) mod redundant_type_annotation;
    pub(crate) mod remove_trailing_return;
    pub(crate) mod remove_unnecessary_else;
    pub(crate) mod return_outside_function;
    pub(crate) mod shadowed_binding;
    pub(crate) mod type_mismatch;
    pub(crate) mod undeclared_effect;
    pub(crate) mod undeclared_fixity;
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
    MappingBindingMismatch, MismatchedArgCount, MismatchedParamCount, MissingEffectAnnotation,
    MissingFields, NoOverloading, NonContiguousSubrange, NotAFunction, RedundantMatchArm,
    RedundantTypeAnnotation, RemoveTrailingReturn, RemoveUnnecessaryElse, ReturnOutsideFunction,
    ShadowedBinding, TypeMismatch, UndeclaredEffect, UndeclaredMappingType, UnnecessaryMutability,
    UnreachableCode, UnresolvedField, UnresolvedIdent, UnresolvedInclude, UnresolvedQuants,
    UnsolvedConstraint, UnusedImport, UnusedVariable, VectorSubrangeOrder,
};
use ide_db::assists::{Assist, AssistId, AssistKind, AssistResolveStrategy, Label, SnippetCap};
use ide_db::line_index::TextRange;
use ide_db::source_change::SourceChange;

/// Extract a `FileRange` from an `InFile<SyntaxNodePtr>`.
///
/// Convenience helper so handlers can write `node_file_range(&d.node)` instead
/// of constructing `FileRange { file_id: d.node.file_id, range: d.node.value.text_range() }`.
pub(crate) fn node_file_range(node: &hir::InFile<syntax::SyntaxNodePtr>) -> hir::FileRange {
    hir::FileRange { file_id: node.file_id, range: node.value.text_range() }
}

// Re-export Severity from hir-def (single source of truth).
pub use hir::diagnostics::Severity;

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
        default: hir::diagnostics::Severity,
    ) -> hir::diagnostics::Severity {
        use hir::diagnostics::Severity;
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
    pub code: hir::diagnostics::DiagnosticCode,
    pub message: String,
    pub range: hir::FileRange,
    pub severity: Severity,
    pub unused: bool,
    pub experimental: bool,
    /// Fix actions attached to this diagnostic.
    pub fixes: Option<Vec<Assist>>,
    /// The syntax node this diagnostic is attached to.
    ///
    /// Used for `#[allow]` attribute resolution in RA; in Sail, used
    /// for precise range adjustment in handlers.
    pub main_node: Option<hir::InFile<syntax::SyntaxNodePtr>>,
    /// Secondary places, each with what it contributes.
    ///
    /// Upstream draws a caret at every one of them — "derived from here",
    /// "bound here" — and a constraint error is mostly the question of
    /// *which argument* is at fault. This used to be appended to the
    /// message as `constraint from 110..111`, which shows the reader two
    /// integers.
    pub related: Vec<(hir::FileRange, String)>,
}

impl Diagnostic {
    /// Create a new diagnostic.
    pub fn new(
        code: hir::diagnostics::DiagnosticCode,
        message: impl Into<String>,
        range: impl Into<hir::FileRange>,
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
            related: Vec::new(),
        }
    }

    /// Attach the places this finding was derived from.
    pub fn with_related(mut self, related: Vec<(hir::FileRange, String)>) -> Self {
        self.related = related;
        self
    }

    /// Create a diagnostic from a syntax node pointer, computing the display range.
    pub(crate) fn new_with_syntax_node_ptr(
        ctx: &DiagnosticsContext<'_>,
        code: hir::diagnostics::DiagnosticCode,
        message: impl Into<String>,
        node: hir::InFile<syntax::SyntaxNodePtr>,
    ) -> Self {
        let file_range = ctx.sema.diagnostics_display_range(node);
        Diagnostic::new(
            code,
            message,
            hir::FileRange { file_id: node.file_id, range: file_range.range },
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
    pub fn with_main_node(mut self, main_node: hir::InFile<syntax::SyntaxNodePtr>) -> Self {
        self.main_node = Some(main_node);
        self
    }
}

/// Context for computing diagnostics for a file.
///
/// Private struct -- visible to handlers via Rust module hierarchy.
struct DiagnosticsContext<'a> {
    file: Option<base_db::FileText>,
    config: &'a DiagnosticsConfig,
    sema: hir::Semantics<'a>,
    /// Whether a fix's edit is built now or left for
    /// `codeAction/resolve`. Read once, by `drop_unresolved_fixes`.
    resolve: &'a AssistResolveStrategy,
}

impl<'a> DiagnosticsContext<'a> {
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
    /// Explicit signatures, distinct from definitions with inferred types.
    pub value_spec_names: std::collections::HashSet<String>,
    pub mapping_spec_names: std::collections::HashSet<String>,
    /// Constructor names (enum/union members) across all workspace files.
    pub constructor_names: std::collections::HashSet<String>,
    /// Value names (let bindings, registers) across all workspace files.
    pub value_names: std::collections::HashSet<String>,
    /// Record/struct names that have known field definitions elsewhere.
    pub record_names: std::collections::HashSet<String>,
    /// Names declared as `scattered function|mapping|union|enum` anywhere in
    /// the workspace. A matching `end` may live in a different file.
    pub scattered_heads: std::collections::HashSet<String>,
    /// Every type name in the workspace: synonyms, structs, unions, enums,
    /// bitfields, newtypes. Used to tell a type-level constant apart from a
    /// value where a numeric type expression is required.
    pub type_names: std::collections::HashSet<String>,
    /// An `$include_error` a workspace file reaches under the current
    /// preprocessor symbols, together with the include-path file names that
    /// transitively lead to it. A source file is responsible for the error
    /// only when one of its own `$include`s names one of those files — this
    /// is how `lib/vector.sail` enforces that a `default Order` has been set.
    pub include_error: Option<(String, std::collections::HashSet<String>)>,
    /// Values from the model configuration named by `$option --config`,
    /// flattened to dotted keys. The value is rendered as JSON so a check can
    /// tell a string from a number without re-parsing.
    pub config_values: std::collections::HashMap<String, String>,
    /// The files on an `$include` cycle, with the cycle rendered for the
    /// message. A cycle is a property of the graph, not of any one file, so
    /// only a caller holding the whole workspace can find it — and a file
    /// reports it only when it is one of the files on the cycle.
    pub include_cycle: Option<(std::collections::HashSet<base_db::FileId>, String)>,
    /// Scattered groups that are incomplete across the whole workspace. A
    /// head in one file and its `end` in another is legal, so a per-file
    /// view can only guess; this is computed once over every item tree.
    pub scattered_problems: Vec<hir::scattered::ScatteredDiagnostic>,
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
    if let Some(max) = config.max_diagnostics_per_file {
        res.truncate(max);
    }
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
        for err in errors.iter().take(config.max_diagnostics_per_file.unwrap_or(usize::MAX)) {
            res.push(Diagnostic::new(
                hir::diagnostics::DiagnosticCode::SyntaxError,
                format!("Syntax Error: {err}"),
                err.range(),
            ));
        }
    }

    // Phase 2: an operator with no fixity. Upstream refuses the program
    // (`initial_check.ml:308`); the parser here recovers, so the report
    // has to come from somewhere that knows the workspace's own table.
    if base_db::preprocess::project_membership(db, ft.file_id(db)) != Some(false) {
        res.extend(handlers::undeclared_fixity::undeclared_fixity(db, file));
    }

    // Phase 3: ItemTree checks (Sail-specific). Bracket matching moved to
    // `syntax::validation`, so it arrives through phase 1.
    let parse_diags = compute_parse_diagnostics(file);
    res.extend(parse_diags.into_iter().map(|d| hir_diag_to_ide(&d, ft.file_id(db))));
    if let Some(max) = config.max_diagnostics_per_file {
        res.truncate(max);
    }

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
    if base_db::preprocess::project_membership(db, ft.file_id(db)) == Some(false) {
        // Like RA's unlinked-file diagnostic, report the missing analysis
        // context once instead of diagnosing a program we have not loaded.
        let code = "unlinked-file";
        return if config.disabled.contains(code) {
            Vec::new()
        } else {
            vec![Diagnostic::new(
                hir::diagnostics::DiagnosticCode::SailLint(code, Severity::Warning),
                "This file is not part of a loaded Sail project. Project-dependent diagnostics are unavailable. Set sail-lsp.workspace.projectFile or add the file to the project's manifest/includes.",
                hir::FileRange { file_id: ft.file_id(db), range: base_db::text_range(0, 0) },
            ).with_severity(config.effective_severity(code, Severity::Warning))]
        };
    }
    let sema = hir::Semantics::new(db);
    let ctx = DiagnosticsContext { file: Some(ft), config, sema, resolve };
    let mut res = Vec::new();
    // `ctx.resolve` is read at the end of this function, once, over every
    // fix — see `drop_unresolved_fixes`.

    // What name resolution found while building the file's `DefMap`. These
    // were collected and never read — `ide-diagnostics` re-checked the same
    // things by walking the CST instead.
    let mut any_from_defs: Vec<AnyDiagnostic> = Vec::new();
    if let (Some(dm), Some(tree)) = (
        hir::def_query::file_def_map(db, ft).as_ref(),
        hir::def_query::file_item_tree(db, ft).as_deref(),
    ) {
        any_from_defs = AnyDiagnostic::from_def_map(&dm.0, tree, ft.file_id(db));
    }

    // Repeated unguarded includes are distinct declarations, not one
    // deduplicated physical file. Sail rejects repeated bodies and type definitions.
    if let Some(workspace) = base_db::WorkspaceFiles::try_get(db) {
        let source = base_db::preprocess::physical_file(db, ft.file_id(db));
        if base_db::preprocess::file_instances(db, source).len() > 1 {
            let mut previous = std::collections::HashMap::new();
            for &view in workspace.file_texts(db) {
                if base_db::preprocess::physical_file(db, view.file_id(db)) != source {
                    continue;
                }
                let Some(tree) = hir::def_query::file_item_tree(db, view).as_ref() else {
                    continue;
                };
                let Some(root) = syntax::parse_query::semantic_parse_file(db, view).syntax_node()
                else {
                    continue;
                };
                for item in tree.top_level_items() {
                    let creating = match item {
                        hir::DefItem::Function(id) if !tree.function(*id).is_clause => "function",
                        hir::DefItem::TypeDef(id) if !tree.type_def(*id).is_clause => {
                            if item.def_kind(tree) == hir::DefKind::TypeAlias {
                                "type synonym"
                            } else {
                                "type"
                            }
                        }
                        hir::DefItem::Register(_) => "register",
                        _ => continue,
                    };
                    let span = item.span(tree);
                    let Some(node) =
                        root.children().find(|n| usize::from(n.text_range().start()) == span.start)
                    else {
                        continue;
                    };
                    let location = hir::InFile {
                        file_id: view.file_id(db),
                        value: syntax::SyntaxNodePtr::new(&node),
                    };
                    let name = item.name(tree).to_string();
                    if let Some(first) = previous.get(&name) {
                        if view == ft {
                            any_from_defs.push(AnyDiagnostic::DuplicateDefinition(
                                DuplicateDefinition {
                                    name: name.clone(),
                                    node: location,
                                    creating,
                                    first: Some(*first),
                                },
                            ));
                        }
                    } else {
                        previous.insert(name, location);
                    }
                }
                if view == ft {
                    break;
                }
            }
        }
    }

    if let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(db) {
        let plan = hir::preprocess::workspace_plan(db, project);
        for (file, event, path) in &plan.missing {
            if *file != ft.file_id(db) {
                continue;
            }
            if let Some(node) =
                syntax::parse_query::parse_file(db, ft).syntax_node().and_then(|root| {
                    root.children()
                        .filter(|n| syntax::preprocess::Event::from_node(n).is_some())
                        .nth(*event)
                })
            {
                any_from_defs.push(AnyDiagnostic::UnresolvedInclude(UnresolvedInclude {
                    path: path.display().to_string(),
                    node: hir::InFile { file_id: *file, value: syntax::SyntaxNodePtr::new(&node) },
                }));
            }
        }
    }

    // An `$include` cycle, found by whoever holds the whole graph.
    if let Some((files, desc)) = workspace_names.and_then(|ws| ws.include_cycle.as_ref()) {
        if files.contains(&ft.file_id(db)) {
            any_from_defs.push(AnyDiagnostic::CircularInclude(hir::diagnostics::CircularInclude {
                cycle_description: desc.clone(),
                node: hir::InFile {
                    file_id: ft.file_id(db),
                    value: syntax::SyntaxNodePtr::from_range(
                        parser::SyntaxKind::ERROR,
                        base_db::text_range(0, 0),
                    ),
                },
            }));
        }
    }

    // Scattered groups the workspace view found incomplete.
    //
    // Only an orphan `end` is reported. Upstream accepts a scattered
    // definition that is never ended — the group simply stays open, and a
    // `match` on it reports `<future instruction clause>` unmatched instead
    // — so `MissingEnd` and `NoClauses` would fire on files `sail
    // --just-check` takes. `end x` with no `scattered x` anywhere is a type
    // error there: "x is not a scattered definition, so it cannot be ended".
    // The other two kinds still reach `add_scattered_end`, which offers to
    // write the `end` rather than demanding it.
    let ordered_scattered = semantic::ordered_scattered::diagnostics(db, ft);
    for d in workspace_names.map(|ws| ws.scattered_problems.as_slice()).unwrap_or(&[]) {
        if ordered_scattered.is_some() {
            break;
        }
        if d.at.file != ft.file_id(db)
            || d.kind != hir::scattered::ScatteredDiagnosticKind::OrphanEnd
        {
            continue;
        }
        any_from_defs.push(AnyDiagnostic::IncompleteScattered(
            hir::diagnostics::IncompleteScattered {
                name: d.name.clone(),
                kind: match d.scattered_kind {
                    Some(hir::scattered::ScatteredKind::Function) => "function",
                    Some(hir::scattered::ScatteredKind::Mapping) => "mapping",
                    Some(hir::scattered::ScatteredKind::Union) => "union",
                    Some(hir::scattered::ScatteredKind::Enum) => "enum",
                    None => "definition",
                }
                .to_owned(),
                problem: d.kind.clone(),
                node: hir::InFile {
                    file_id: d.at.file,
                    value: syntax::SyntaxNodePtr::from_range(
                        parser::SyntaxKind::ERROR,
                        base_db::text_range(d.at.span.start, d.at.span.end),
                    ),
                },
            },
        ));
    }

    // AST visitor checks.
    res.extend(ordered_scattered.into_iter().flatten());
    let semantic_diags = semantic::compute_semantic_diagnostics_with_names(file, workspace_names);
    for d in &semantic_diags {
        res.push(hir_diag_to_ide(d, ft.file_id(db)));
    }

    // Per-callable inference diagnostics.
    let callable_ids = hir::def_query::file_def_with_body_ids(db, ft);
    let mut any_diags: Vec<AnyDiagnostic> = any_from_defs;

    for &id in callable_ids {
        let tcr = hir::ty_query::infer(db, id);

        // InferenceDiagnostic -> AnyDiagnostic
        let typed_diags = tcr.0.inference_diagnostics();
        let has_typed_diags = !typed_diags.is_empty();
        let has_type_mismatches = !tcr.0.type_mismatches.is_empty();

        if has_typed_diags || has_type_mismatches {
            let bsm = hir::def_query::body_with_source_map(db, id);
            let source_map = &bsm.0 .1;

            for diag in typed_diags {
                if let Some(d) = AnyDiagnostic::inference_diagnostic(
                    diag,
                    source_map,
                    tcr.0.def_base,
                    &tcr.0.source_tokens,
                ) {
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
    if let Some(ws) =
        workspace_names.filter(|_| hir::source_order::declarations_for(db, ft).is_none())
    {
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
            AnyDiagnostic::MismatchedParamCount(d) =>
                handlers::mismatched_param_count::mismatched_param_count(&ctx, d),
            AnyDiagnostic::ExpectedFunction(d) =>
                handlers::expected_function::expected_function(&ctx, d),
            AnyDiagnostic::EffectViolation(d) =>
                handlers::effect_violation::effect_violation(&ctx, d),
            AnyDiagnostic::IncompleteMatch(d) =>
                handlers::missing_match_arms::missing_match_arms(&ctx, d),
            AnyDiagnostic::RedundantMatchArm(d) =>
                handlers::redundant_match_arm::redundant_match_arm(&ctx, d),
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
            AnyDiagnostic::InvalidTypeUse(d) => invalid_type_use(&ctx, d),
            AnyDiagnostic::UnrecoverableImplicit(d) => unrecoverable_implicit(d),
            AnyDiagnostic::NoOverloading(d) =>
                handlers::no_overloading::no_overloading(&ctx, d),
            AnyDiagnostic::NotAFunction(d) =>
                handlers::not_a_function::not_a_function(&ctx, d),
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

    drop_unresolved_fixes(ctx.resolve, &mut res);
    res
}

/// Drop the edit from every fix the caller did not ask to resolve.
///
/// rust-analyzer's `AssistResolveStrategy`: a client that advertises
/// `codeAction/resolve` gets the *label* now and the edit when it asks, so
/// a request that only needs to know a fix exists does not pay for
/// building it. `should_resolve` had no caller here, and the strategy was
/// hard-coded to `All` at the one place that chose one — while the
/// publish path, which reads no fix at all, was the caller paying for it.
///
/// The fix survives as a label; only the `SourceChange` goes.
fn drop_unresolved_fixes(resolve: &AssistResolveStrategy, res: &mut [Diagnostic]) {
    if matches!(resolve, AssistResolveStrategy::All) {
        return;
    }
    for d in res.iter_mut() {
        let Some(fixes) = &mut d.fixes else { continue };
        for fix in fixes.iter_mut() {
            if !resolve.should_resolve(&fix.id) {
                fix.source_change = None;
            }
        }
    }
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
            // A record whose declaration this handle could not see: field
            // resolution fails per-file and would succeed workspace-wide.
            //
            // This used to suppress *every* miss whose receiver was a known
            // record name — which is the case where the fields are known and
            // the miss is real, so `r.zzz` on a struct declared right beside
            // it was never reported. `record_names` is workspace-wide, so it
            // could not tell "declared elsewhere" from "declared here".
            !d.receiver_fields_known
                && d.receiver.as_name().map(|n| ws.record_names.contains(n)).unwrap_or(false)
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

/// Convert `hir::diagnostics::Diagnostic` to `ide_diagnostics::Diagnostic`.
fn hir_diag_to_ide(d: &hir::diagnostics::Diagnostic, file: base_db::FileId) -> Diagnostic {
    let unused = d.tags.iter().any(|t| matches!(t, hir::diagnostics::DiagnosticTag::Unnecessary));
    let related = d
        .related
        .iter()
        .map(|(r, msg)| (hir::FileRange { file_id: file, range: *r }, msg.clone()))
        .chain(d.related_in_files.iter().map(|(file, range, message)| {
            (hir::FileRange { file_id: *file, range: *range }, message.clone())
        }))
        .collect();
    Diagnostic::new(d.code.clone(), d.message.clone(), d.range)
        .with_severity(d.severity)
        .with_unused(unused)
        .with_related(related)
}

// Kept for unit tests that need to dispatch a single AnyDiagnostic.

fn invalid_type_use(
    ctx: &DiagnosticsContext<'_>,
    d: &hir::diagnostics::InvalidTypeUse,
) -> Diagnostic {
    let mut range = node_file_range(&d.node);
    if let Some(name) = &d.unbound_variable {
        let token = ctx
            .file
            .filter(|file| file.file_id(ctx.db()) == d.node.file_id)
            .and_then(|file| ctx.sema.parse(file))
            .and_then(|root| d.node.value.try_to_node(&root))
            .and_then(|node| {
                node.descendants_with_tokens().filter_map(|el| el.into_token()).find(|token| {
                    token.kind() == syntax::SyntaxKind::TY_VAR && token.text() == name
                })
            });
        if let Some(token) = token {
            range.range = token.text_range();
        }
    }
    // `p as typ` errors concern the type pattern, not the value binder.
    if d.node.value.kind() == syntax::SyntaxKind::AS_PAT {
        let pattern = ctx
            .file
            .filter(|file| file.file_id(ctx.db()) == d.node.file_id)
            .and_then(|file| ctx.sema.parse(file))
            .and_then(|root| d.node.value.try_to_node(&root))
            .and_then(|node| node.children().last());
        if let Some(mut pattern) = pattern {
            if let Some(path) = &d.type_pattern_path {
                for &index in path {
                    let child = match pattern.kind() {
                        syntax::SyntaxKind::TYPE_APP => pattern.children().nth(index + 1),
                        syntax::SyntaxKind::TYPE_TUPLE => pattern.children().nth(index),
                        _ => None,
                    };
                    let Some(child) = child else { break };
                    pattern = child;
                }
            }
            range.range = pattern.text_range();
        }
    }
    Diagnostic::new(
        hir::diagnostics::DiagnosticCode::SailError("type-error"),
        d.message.clone(),
        range,
    )
}

fn unrecoverable_implicit(d: &hir::diagnostics::InvalidTypeUse) -> Diagnostic {
    Diagnostic::new(
        hir::diagnostics::DiagnosticCode::SailError("unrecoverable-implicit"),
        d.message.clone(),
        node_file_range(&d.node),
    )
}

#[cfg(test)]
pub(crate) fn dispatch_one(ctx: &DiagnosticsContext<'_>, d: &AnyDiagnostic) -> Option<Diagnostic> {
    Some(match d {
        AnyDiagnostic::UnresolvedIdent(d) => handlers::unresolved_ident::unresolved_ident(ctx, d),
        AnyDiagnostic::UnresolvedField(d) => handlers::unresolved_field::unresolved_field(ctx, d),
        AnyDiagnostic::MismatchedArgCount(d) => {
            handlers::mismatched_arg_count::mismatched_arg_count(ctx, d)
        }
        AnyDiagnostic::MismatchedParamCount(d) => {
            handlers::mismatched_param_count::mismatched_param_count(ctx, d)
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
        AnyDiagnostic::InvalidTypeUse(d) => invalid_type_use(ctx, d),
        AnyDiagnostic::UnrecoverableImplicit(d) => unrecoverable_implicit(d),
        AnyDiagnostic::NoOverloading(d) => handlers::no_overloading::no_overloading(ctx, d),
        AnyDiagnostic::NotAFunction(d) => handlers::not_a_function::not_a_function(ctx, d),
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
        AnyDiagnostic::RedundantMatchArm(d) => {
            handlers::redundant_match_arm::redundant_match_arm(ctx, d)
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
            diag.severity = hir::diagnostics::Severity::WeakWarning;
        } else if config.warnings_as_info.contains(code) {
            diag.severity = hir::diagnostics::Severity::Information;
        }
    }

    // Filter out Allow-severity diagnostics.
    diagnostics.retain(|d| {
        d.severity != hir::diagnostics::Severity::Hint || {
            // Keep hints that are explicitly enabled.
            true
        }
    });
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
        import: None,
        id: AssistId(id, AssistKind::QuickFix),
        label: Label::new(label.to_owned()),
        group: None,
        target,
        source_change: None,
        command: None,
    }
}

// Tests moved to tests.rs.
