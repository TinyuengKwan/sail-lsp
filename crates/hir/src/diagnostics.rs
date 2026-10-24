//! HIR-level diagnostic types and cooking.
//! `AnyDiagnostic` is the central enum collecting all diagnostic kinds.
//! Each variant holds resolved data (`InFile<SyntaxNodePtr>`, `Ty`)
//! rather than raw HIR IDs. The `ide-diagnostics` crate consumes this
//! enum to produce LSP diagnostics via per-variant handlers.
//!
//! `inference_diagnostic()` converts raw `InferenceDiagnostic` (ExprId/PatId)
//! to `AnyDiagnostic` using `BodySourceMap`.

pub use hir_def::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticTag, Severity};

use hir_def::body::BodySourceMap;
use hir_def::in_file::InFile;
use hir_def::EffectTag;
use hir_ty::infer::{InferenceDiagnostic, Ty};
use syntax::SyntaxNodePtr;

/// Union of all resolved diagnostics from the HIR layer.
///
/// Each variant wraps a per-diagnostic struct with resolved position
/// (`InFile<SyntaxNodePtr>`) and type data (`Ty`).
pub enum AnyDiagnostic {
    UnresolvedIdent(UnresolvedIdent),
    UnresolvedField(UnresolvedField),
    MismatchedArgCount(MismatchedArgCount),
    MismatchedParamCount(MismatchedParamCount),
    ExpectedFunction(ExpectedFunction),
    EffectViolation(EffectViolation),
    IncompleteMatch(IncompleteMatch),
    RedundantMatchArm(RedundantMatchArm),
    MissingFields(MissingFields),
    UnsolvedConstraint(UnsolvedConstraint),
    TypeMismatch(TypeMismatch),
    UnusedVariable(UnusedVariable),
    RemoveTrailingReturn(RemoveTrailingReturn),
    RemoveUnnecessaryElse(RemoveUnnecessaryElse),
    // Name resolution + scope diagnostics
    UnresolvedInclude(UnresolvedInclude),
    DuplicateDefinition(DuplicateDefinition),
    IncompleteScattered(IncompleteScattered),
    CircularInclude(CircularInclude),
    ShadowedBinding(ShadowedBinding),
    // Type system diagnostics
    ReturnOutsideFunction(ReturnOutsideFunction),
    InvalidVectorConcat(InvalidVectorConcat),
    InvalidListPattern(InvalidListPattern),
    InvalidStringPattern(InvalidStringPattern),
    ImpossibleConstraint(ImpossibleConstraint),
    UnresolvedQuants(UnresolvedQuants),
    InvalidSliceAssign(InvalidSliceAssign),
    UndeclaredMappingType(UndeclaredMappingType),
    // Lint / Warning diagnostics
    UnusedImport(UnusedImport),
    UnnecessaryMutability(UnnecessaryMutability),
    DeprecatedSyntax(DeprecatedSyntax),
    InconsistentHexCasing(InconsistentHexCasing),
    UnreachableCode(UnreachableCode),
    RedundantTypeAnnotation(RedundantTypeAnnotation),
    // Effect system diagnostics
    MissingEffectAnnotation(MissingEffectAnnotation),
    UndeclaredEffect(UndeclaredEffect),
    EffectMismatchInOverride(EffectMismatchInOverride),
    // Sail-specific variants
    ConcatTypeMismatch(ConcatTypeMismatch),
    InvalidTypeUse(InvalidTypeUse),
    UnrecoverableImplicit(InvalidTypeUse),
    NoOverloading(NoOverloading),
    NotAFunction(NotAFunction),
    DuplicateBinding(DuplicateBindingDiag),
    NonContiguousSubrange(NonContiguousSubrange),
    VectorSubrangeOrder(VectorSubrangeOrder),
    MappingBindingMismatch(MappingBindingMismatch),
}

pub struct UnresolvedIdent {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct UnresolvedField {
    pub receiver: Ty,
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
    /// See `InferenceDiagnostic::UnresolvedField`.
    pub receiver_fields_known: bool,
    /// See `InferenceDiagnostic::UnresolvedField`.
    pub receiver_is_not_a_record: bool,
    /// See `InferenceDiagnostic::UnresolvedField`.
    pub in_assign_target: bool,
}

pub struct MismatchedArgCount {
    pub expected: usize,
    pub found: usize,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct MismatchedParamCount {
    pub expected: usize,
    pub found: usize,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct ExpectedFunction {
    pub found: Ty,
    pub node: InFile<SyntaxNodePtr>,
}

/// Sail-specific: effect purity violation.
pub struct EffectViolation {
    pub effect: EffectTag,
    pub context: &'static str,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct IncompleteMatch {
    pub missing_arms: Vec<String>,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct MissingFields {
    pub record_name: String,
    pub missing: Vec<String>,
    /// Type names of the missing fields (for smart-fill defaults). .
    pub field_types: Option<Vec<String>>,
    pub node: InFile<SyntaxNodePtr>,
}

/// Sail-specific: numeric constraint unsolvable.
pub struct UnsolvedConstraint {
    pub constraint: String,
    /// Whether the constraint was *proved* unsatisfiable, as opposed to left
    /// undecided. `'m == 8 * 'n` with `'m` fixed at 65 has no integer
    /// solution — that is an error, where "the solver did not get there" is
    /// a warning.
    pub proved_false: bool,
    /// Source spans the constraint was derived from — the call arguments that
    /// produced it. Empty when the constraint has no recorded provenance.
    pub derived_from: Vec<parser::Span>,
    pub node: InFile<SyntaxNodePtr>,
}

/// Type mismatch between expected and actual types.
pub struct TypeMismatch {
    pub expected: Ty,
    pub actual: Ty,
    pub expr_or_pat: InFile<SyntaxNodePtr>,
    /// Sail-specific: constraint origin tracking.
    pub expected_source: Option<String>,
}

pub struct UnusedVariable {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct RemoveTrailingReturn {
    pub node: InFile<SyntaxNodePtr>,
}

pub struct RemoveUnnecessaryElse {
    pub node: InFile<SyntaxNodePtr>,
}

pub struct ConcatTypeMismatch {
    pub message: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct InvalidTypeUse {
    pub message: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct NoOverloading {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
    /// Nothing is bound under this name at all, which upstream words as
    /// "Function {name} not found" rather than as an overload failure.
    pub no_function: bool,
    /// The nearest name the environment knows, if one is near enough.
    pub closest: Option<String>,
}

/// A name used as a function that is bound to something else.
pub struct NotAFunction {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct DuplicateBindingDiag {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
    /// A duplicate field in a struct pattern rather than a duplicate
    /// binding; upstream words the two differently.
    pub in_struct_pattern: bool,
}

pub struct NonContiguousSubrange {
    pub name: String,
    pub problem: hir_ty::infer::SubrangeProblem,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct VectorSubrangeOrder {
    pub first: String,
    pub second: String,
    pub order_desc: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct MappingBindingMismatch {
    pub name: String,
    pub side: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// $include path could not be resolved.
pub struct UnresolvedInclude {
    pub path: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct DuplicateDefinition {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
    /// The word upstream uses for what the second definition creates:
    /// "Cannot create {kind} {name}, name is already bound".
    pub creating: &'static str,
    /// Where the name was bound first. Upstream draws a caret there too,
    /// hinted "previous binding" (`type_env.ml:430`).
    pub first: Option<InFile<SyntaxNodePtr>>,
}

/// Scattered definition missing `end` marker (Sail-specific).
pub struct IncompleteScattered {
    pub name: String,
    pub kind: String,
    /// Which of the three ways a scattered group can be incomplete.
    pub problem: hir_def::scattered::ScatteredDiagnosticKind,
    pub node: InFile<SyntaxNodePtr>,
}

/// Circular $include dependency.
pub struct CircularInclude {
    pub cycle_description: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Variable binding shadows an existing binding in scope.
pub struct ShadowedBinding {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// `return` used outside any function body.
pub struct ReturnOutsideFunction {
    pub node: InFile<SyntaxNodePtr>,
}

/// Empty vector concatenation or non-vector operand in `@` operator.
pub struct InvalidVectorConcat {
    pub message: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// `cons` pattern used against non-list type.
pub struct InvalidListPattern {
    pub found: Ty,
    pub node: InFile<SyntaxNodePtr>,
}

/// String-append pattern used against non-string type.
pub struct InvalidStringPattern {
    pub found: Ty,
    pub node: InFile<SyntaxNodePtr>,
}

/// Function clause has contradictory/impossible type constraints.
pub struct ImpossibleConstraint {
    pub constraint: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Universally quantified type variables could not be resolved.
pub struct UnresolvedQuants {
    pub name: String,
    pub constraint: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Slice assignment to non-vector type.
pub struct InvalidSliceAssign {
    pub found: Ty,
    pub node: InFile<SyntaxNodePtr>,
}

/// Mapping definition without a declared type.
pub struct UndeclaredMappingType {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// $include that is never used by any definition.
pub struct UnusedImport {
    pub path: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Mutable variable (`var`) that is never modified.
pub struct UnnecessaryMutability {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Use of deprecated syntax (e.g., explicit effect annotations).
pub struct DeprecatedSyntax {
    pub message: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Hex literal with inconsistent casing (e.g., `0xaBcD`).
pub struct InconsistentHexCasing {
    pub literal: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// A match arm an earlier arm already covers.
pub struct RedundantMatchArm {
    pub node: InFile<SyntaxNodePtr>,
}

/// Code after a diverging expression (return/throw/exit).
pub struct UnreachableCode {
    pub node: InFile<SyntaxNodePtr>,
}

/// Type annotation that adds no information (inferable).
/// Hint severity — informational, not a problem.
pub struct RedundantTypeAnnotation {
    pub node: InFile<SyntaxNodePtr>,
}

/// Function observes effects but has no effect annotation.
pub struct MissingEffectAnnotation {
    pub name: String,
    pub effects: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Function uses an effect that is not declared in its signature.
pub struct UndeclaredEffect {
    pub name: String,
    pub effect: String,
    pub node: InFile<SyntaxNodePtr>,
}

/// Mapping forward/backward clauses have different effect sets.
pub struct EffectMismatchInOverride {
    pub name: String,
    pub expected: String,
    pub found: String,
    pub node: InFile<SyntaxNodePtr>,
}

impl AnyDiagnostic {
    /// Convert raw `InferenceDiagnostic` to resolved `AnyDiagnostic`.
    pub fn inference_diagnostic(
        d: &InferenceDiagnostic,
        source_map: &BodySourceMap,
        // Where the callable started when `d` was produced. Equal to the
        // source map's own base unless the result was reused across an edit
        // above the callable.
        def_base: usize,
    ) -> Option<AnyDiagnostic> {
        // Helper closures for syntax lookup.
        let expr_syntax = |expr| source_map.expr_syntax_in_file(expr);
        let pat_syntax = |pat| source_map.pat_syntax_in_file(pat);

        match d {
            InferenceDiagnostic::InvalidTypeUse { node, message } => {
                let node = match node {
                    hir_def::ExprOrPatId::ExprId(id) => expr_syntax(*id)?,
                    hir_def::ExprOrPatId::PatId(id) => pat_syntax(*id)?,
                };
                Some(AnyDiagnostic::InvalidTypeUse(InvalidTypeUse {
                    node,
                    message: message.clone(),
                }))
            }
            InferenceDiagnostic::UnresolvedIdent { expr, name } => {
                Some(AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
                    name: name.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::UnresolvedField {
                expr,
                receiver,
                name,
                receiver_fields_known,
                receiver_is_not_a_record,
                in_assign_target,
                ..
            } => Some(AnyDiagnostic::UnresolvedField(UnresolvedField {
                receiver: receiver.clone(),
                name: name.clone(),
                node: expr_syntax(*expr)?,
                receiver_fields_known: *receiver_fields_known,
                receiver_is_not_a_record: *receiver_is_not_a_record,
                in_assign_target: *in_assign_target,
            })),
            InferenceDiagnostic::MismatchedArgCount { call_expr, expected, found } => {
                Some(AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
                    expected: *expected,
                    found: *found,
                    node: expr_syntax(*call_expr)?,
                }))
            }
            InferenceDiagnostic::MismatchedParamCount { body_expr, expected, found } => {
                Some(AnyDiagnostic::MismatchedParamCount(MismatchedParamCount {
                    expected: *expected,
                    found: *found,
                    node: expr_syntax(*body_expr)?,
                }))
            }
            InferenceDiagnostic::ExpectedFunction { call_expr, found } => {
                Some(AnyDiagnostic::ExpectedFunction(ExpectedFunction {
                    found: found.clone(),
                    node: expr_syntax(*call_expr)?,
                }))
            }
            InferenceDiagnostic::EffectViolation { expr, effect, context } => {
                Some(AnyDiagnostic::EffectViolation(EffectViolation {
                    effect: *effect,
                    context,
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::RedundantMatchArm { pat } => {
                Some(AnyDiagnostic::RedundantMatchArm(RedundantMatchArm {
                    node: pat_syntax(*pat)?,
                }))
            }
            InferenceDiagnostic::IncompleteMatch { expr, missing_arms } => {
                Some(AnyDiagnostic::IncompleteMatch(IncompleteMatch {
                    missing_arms: missing_arms.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::MissingFields { expr, record_name, missing } => {
                Some(AnyDiagnostic::MissingFields(MissingFields {
                    record_name: record_name.clone(),
                    missing: missing.clone(),
                    field_types: None, // TODO: populate from struct definition
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::UnsolvedConstraint { expr, constraint } => {
                Some(AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
                    constraint: constraint.clone(),
                    proved_false: false,
                    derived_from: Vec::new(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::UnusedVariable { pat, name } => {
                Some(AnyDiagnostic::UnusedVariable(UnusedVariable {
                    name: name.clone(),
                    node: pat_syntax(*pat)?,
                }))
            }
            InferenceDiagnostic::RemoveTrailingReturn { return_expr } => {
                Some(AnyDiagnostic::RemoveTrailingReturn(RemoveTrailingReturn {
                    node: expr_syntax(*return_expr)?,
                }))
            }
            InferenceDiagnostic::UnreachableStatement { expr } => {
                Some(AnyDiagnostic::UnreachableCode(UnreachableCode { node: expr_syntax(*expr)? }))
            }
            InferenceDiagnostic::RemoveUnnecessaryElse { if_expr } => {
                Some(AnyDiagnostic::RemoveUnnecessaryElse(RemoveUnnecessaryElse {
                    node: expr_syntax(*if_expr)?,
                }))
            }
            InferenceDiagnostic::ConcatTypeMismatch { expr, message } => {
                Some(AnyDiagnostic::ConcatTypeMismatch(ConcatTypeMismatch {
                    message: message.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            // Raised only where the index is a literal past a width that is
            // known — `x[8]` on `bits(8)`. That is proved false, not
            // undecided, and upstream rejects the file.
            InferenceDiagnostic::UnrecoverableImplicit { expr, message } => {
                Some(AnyDiagnostic::UnrecoverableImplicit(InvalidTypeUse {
                    node: expr_syntax(*expr)?,
                    message: message.clone(),
                }))
            }
            InferenceDiagnostic::ConstraintViolation { expr, constraint, derived_from } => {
                Some(AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
                    constraint: constraint.clone(),
                    proved_false: true,
                    // Recorded against the revision this result was
                    // inferred in, which need not be the current one.
                    derived_from: derived_from
                        .iter()
                        .map(|s| source_map.rebase(*s, def_base))
                        .collect(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::NoOverloading { call_expr, name, no_function, closest } => {
                Some(AnyDiagnostic::NoOverloading(NoOverloading {
                    name: name.clone(),
                    node: expr_syntax(*call_expr)?,
                    no_function: *no_function,
                    closest: closest.clone(),
                }))
            }
            InferenceDiagnostic::NotAFunction { call_expr, name } => {
                Some(AnyDiagnostic::NotAFunction(NotAFunction {
                    name: name.clone(),
                    node: expr_syntax(*call_expr)?,
                }))
            }
            InferenceDiagnostic::MappingBindingMismatch { expr, name, side } => {
                Some(AnyDiagnostic::MappingBindingMismatch(MappingBindingMismatch {
                    name: name.clone(),
                    side: side.to_string(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::DuplicateBinding { pat, name, in_struct_pattern } => {
                Some(AnyDiagnostic::DuplicateBinding(DuplicateBindingDiag {
                    name: name.clone(),
                    node: pat_syntax(*pat)?,
                    in_struct_pattern: *in_struct_pattern,
                }))
            }
            InferenceDiagnostic::MissingPatternFields { pat, record_name, missing } => {
                Some(AnyDiagnostic::MissingFields(MissingFields {
                    record_name: record_name.clone(),
                    missing: missing.clone(),
                    field_types: None,
                    node: pat_syntax(*pat)?,
                }))
            }
            // The same fact as a field access that does not resolve, and it
            // reads the same way: "no field `zzz` on type `Record`". The
            // record's declaration is what `Pat::Struct` looked the fields
            // up in, so they are known by construction.
            InferenceDiagnostic::UnknownPatternField { pat, record_name, name } => {
                Some(AnyDiagnostic::UnresolvedField(UnresolvedField {
                    receiver: Ty::named(record_name.clone()),
                    name: name.clone(),
                    node: pat_syntax(*pat)?,
                    receiver_fields_known: true,
                    // A struct pattern's record is a record.
                    receiver_is_not_a_record: false,
                    in_assign_target: false,
                }))
            }
            InferenceDiagnostic::NonContiguousSubrange { pat, name, problem } => {
                Some(AnyDiagnostic::NonContiguousSubrange(NonContiguousSubrange {
                    name: name.clone(),
                    problem: problem.clone(),
                    node: pat_syntax(*pat)?,
                }))
            }
            InferenceDiagnostic::VectorSubrangeOrder { node, first, second, order } => {
                let order_desc = match order {
                    hir_def::type_error::VectorOrder::Dec => "default Order dec".to_string(),
                    hir_def::type_error::VectorOrder::Inc => "default Order inc".to_string(),
                };
                let at = match node {
                    hir_def::ExprOrPatId::ExprId(id) => expr_syntax(*id)?,
                    hir_def::ExprOrPatId::PatId(id) => pat_syntax(*id)?,
                };
                Some(AnyDiagnostic::VectorSubrangeOrder(VectorSubrangeOrder {
                    first: first.clone(),
                    second: second.clone(),
                    order_desc,
                    node: at,
                }))
            }
            InferenceDiagnostic::IncorrectCase { .. } => None,
            // A constraint a call site could not discharge. The comment that
            // used to stand here said these were "handled in workspace.rs
            // diagnostic rendering" — that renderer
            // (`cook_inference_diagnostics_to_legacy`) is only reachable from
            // the non-salsa `check_file*` entry points, which the server never
            // calls, so returning `None` here dropped them entirely.
            InferenceDiagnostic::CallConstraintViolation {
                call_expr,
                constraint,
                derived_from,
                proved_false,
            } => {
                if std::env::var("CONSTRPROBE").is_ok() {
                    eprintln!(
                        "CONSTR3 cooking {constraint:?} node={:?}",
                        expr_syntax(*call_expr).is_some()
                    );
                }
                Some(AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
                    constraint: constraint.clone(),
                    proved_false: *proved_false,
                    derived_from: derived_from
                        .iter()
                        .map(|s| source_map.rebase(*s, def_base))
                        .collect(),
                    node: expr_syntax(*call_expr)?,
                }))
            }
            // Deliberately dropped, and re-measured rather than inherited.
            //
            // 2026-08-04: 9 false positives on typecheck/pass and 1 on
            // sail-riscv, for 3 mutants — the wrong side of the trade.
            // 2026-08-08: the same 9 and the same 1, for **0** mutants. The
            // three it used to buy are caught now by checks that do not
            // guess: the payload arity of a constructor, the kind of a
            // cross-file argument, an enum member in a pattern. So the
            // trade has gone from bad to free of any upside at all.
            //
            // What it needs is a real unifier: division (`int(8 * 'n)`
            // against `int('m)` given `'m == 8 * 'p`), Bool-kinded
            // variables, equivalent-but-distinct unifiers, and implicit
            // arguments recovered from the expected type through an
            // operator. Until then the check cannot be shown to anyone.
            InferenceDiagnostic::UnresolvedCallQuantifiers { .. } => None,
        }
    }

    /// Cook the diagnostics name resolution collected while building a
    /// file's `DefMap`.
    ///
    /// These were collected and never read: `DefMap::diagnostics()` had no
    /// caller outside the CLI and its own tests, so nothing name resolution
    /// found ever reached an editor. `ide-diagnostics` re-implemented the
    /// same checks by walking the CST instead — two pipelines, and the one
    /// the user saw was the one further from the facts.
    pub fn from_def_map(
        def_map: &hir_def::DefMap,
        item_tree: &hir_def::ItemTree,
        file_id: base_db::FileId,
    ) -> Vec<AnyDiagnostic> {
        use hir_def::nameres::DefDiagnostic;
        /// The word upstream puts in "Cannot create _ , name is already
        /// bound". A `newtype` is a union there — `to_ast_typedef` routes
        /// it through the same `already_bound "union"`
        /// (`type_env.ml:1597`) — and a `bitfield` is a struct.
        fn creating_word(kind: hir_def::item_tree::DefKind) -> &'static str {
            use hir_def::item_tree::DefKind;
            match kind {
                DefKind::Enum => "enum type",
                DefKind::Union | DefKind::Newtype => "union type",
                DefKind::Struct | DefKind::Bitfield => "struct type",
                DefKind::TypeAlias => "type synonym",
                _ => "type",
            }
        }

        let span_of = |idx: hir_def::nameres::DefIdx| -> Option<hir_def::Span> {
            let data = def_map.get(idx)?;
            Some(item_tree.top_level_items().get(data.item_tree_index)?.name_span(item_tree))
        };
        let node_at = |span: hir_def::Span| InFile {
            file_id,
            value: SyntaxNodePtr::from_range(
                parser::SyntaxKind::ERROR,
                base_db::text_range(span.start, span.end),
            ),
        };
        def_map
            .diagnostics()
            .iter()
            .filter_map(|d| match d {
                DefDiagnostic::UnresolvedInclude { path, range } => {
                    Some(AnyDiagnostic::UnresolvedInclude(UnresolvedInclude {
                        path: path.clone(),
                        node: node_at(*range),
                    }))
                }
                // The *second* definition is the one to point at: the first
                // is where the name legitimately came from.
                DefDiagnostic::DuplicateDefinition { name, first, second, kind } => {
                    Some(AnyDiagnostic::DuplicateDefinition(DuplicateDefinition {
                        name: name.to_string(),
                        node: node_at(span_of(*second)?),
                        creating: creating_word(*kind),
                        first: span_of(*first).map(&node_at),
                    }))
                }
                // Reported by the workspace-level scattered check, which can
                // see an `end` in another file. A per-file view cannot.
                DefDiagnostic::IncompleteScattered { .. } => None,
            })
            .collect()
    }

    /// Cook type mismatches from `InferenceResult.type_mismatches`.
    pub fn from_type_mismatches(
        type_mismatches: &rustc_hash::FxHashMap<hir_def::ExprOrPatId, hir_ty::infer::TypeMismatch>,
        source_map: &BodySourceMap,
    ) -> Vec<AnyDiagnostic> {
        let mut result = Vec::new();
        for (expr_or_pat, mismatch) in type_mismatches {
            let node = match expr_or_pat {
                hir_def::ExprOrPatId::ExprId(id) => source_map.expr_syntax_in_file(*id),
                hir_def::ExprOrPatId::PatId(id) => source_map.pat_syntax_in_file(*id),
            };
            if let Some(expr_or_pat_node) = node {
                result.push(AnyDiagnostic::TypeMismatch(TypeMismatch {
                    expected: mismatch.expected.clone(),
                    actual: mismatch.actual.clone(),
                    expr_or_pat: expr_or_pat_node,
                    expected_source: None,
                }));
            }
        }
        result
    }
}
