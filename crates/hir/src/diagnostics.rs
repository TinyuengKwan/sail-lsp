//! HIR-level diagnostic types and cooking.
//! `AnyDiagnostic` is the central enum collecting all diagnostic kinds.
//! Each variant holds resolved data (`InFile<SyntaxNodePtr>`, `Ty`)
//! rather than raw HIR IDs. The `ide-diagnostics` crate consumes this
//! enum to produce LSP diagnostics via per-variant handlers.
//!
//! `inference_diagnostic()` converts raw `InferenceDiagnostic` (ExprId/PatId)
//! to `AnyDiagnostic` using `BodySourceMap`.

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
    ExpectedFunction(ExpectedFunction),
    EffectViolation(EffectViolation),
    IncompleteMatch(IncompleteMatch),
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
    NoOverloading(NoOverloading),
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
}

pub struct MismatchedArgCount {
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

pub struct NoOverloading {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct DuplicateBindingDiag {
    pub name: String,
    pub node: InFile<SyntaxNodePtr>,
}

pub struct NonContiguousSubrange {
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
}

/// Scattered definition missing `end` marker (Sail-specific).
pub struct IncompleteScattered {
    pub name: String,
    pub kind: String,
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
    ) -> Option<AnyDiagnostic> {
        // Helper closures for syntax lookup.
        let expr_syntax = |expr| source_map.expr_syntax_in_file(expr);
        let pat_syntax = |pat| source_map.pat_syntax_in_file(pat);

        match d {
            InferenceDiagnostic::UnresolvedIdent { expr, name } => {
                Some(AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
                    name: name.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::UnresolvedField { expr, receiver, name, .. } => {
                Some(AnyDiagnostic::UnresolvedField(UnresolvedField {
                    receiver: receiver.clone(),
                    name: name.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::MismatchedArgCount { call_expr, expected, found } => {
                Some(AnyDiagnostic::MismatchedArgCount(MismatchedArgCount {
                    expected: *expected,
                    found: *found,
                    node: expr_syntax(*call_expr)?,
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
            InferenceDiagnostic::ConstraintViolation { expr, constraint, .. } => {
                Some(AnyDiagnostic::UnsolvedConstraint(UnsolvedConstraint {
                    constraint: constraint.clone(),
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::NoOverloading { call_expr, name } => {
                Some(AnyDiagnostic::NoOverloading(NoOverloading {
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
            InferenceDiagnostic::DuplicateBinding { pat, name } => {
                Some(AnyDiagnostic::DuplicateBinding(DuplicateBindingDiag {
                    name: name.clone(),
                    node: pat_syntax(*pat)?,
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
            InferenceDiagnostic::NonContiguousSubrange { pat } => {
                Some(AnyDiagnostic::NonContiguousSubrange(NonContiguousSubrange {
                    node: pat_syntax(*pat)?,
                }))
            }
            InferenceDiagnostic::VectorSubrangeOrder { expr, first, second, order } => {
                let order_desc = match order {
                    hir_def::type_error::VectorOrder::Dec => "default Order dec".to_string(),
                    hir_def::type_error::VectorOrder::Inc => "default Order inc".to_string(),
                };
                Some(AnyDiagnostic::VectorSubrangeOrder(VectorSubrangeOrder {
                    first: first.clone(),
                    second: second.clone(),
                    order_desc,
                    node: expr_syntax(*expr)?,
                }))
            }
            InferenceDiagnostic::IncorrectCase { .. } => None,
            // Call-site constraint/quantifier errors are handled in
            // workspace.rs diagnostic rendering, not here.
            InferenceDiagnostic::CallConstraintViolation { .. }
            | InferenceDiagnostic::UnresolvedCallQuantifiers { .. } => None,
        }
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
