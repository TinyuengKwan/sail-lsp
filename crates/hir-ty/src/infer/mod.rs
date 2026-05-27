use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use la_arena::ArenaMap;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

// All imports from hir-def — no ide-db or ide-diagnostics dependency.
use crate::diagnostics::match_check::{self, EnvCx, MatchTy, RecordFields};
use hir_def::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use hir_def::type_error::{TypeError, VectorOrder};

#[cfg(feature = "z3-solver")]
pub mod z3_solver;

pub use crate::cancel::CancellationToken;
use parser::Literal;
use parser::Span;
use syntax::ast::{self, AstNode as _};

mod constraint;
mod fallback;
use constraint::*;
/// Top-level type environment (TopLevelEnv, RecordInfo, BitfieldInfo).
pub mod env;
pub(crate) use env::{BitfieldInfo, RecordInfo, TopLevelEnv};
// Re-export env helper functions used by sibling modules (expr.rs, numeric.rs, z3_solver.rs)
pub(super) use env::{
    apply_callable_signature_metadata, bits_ty, build_env_from_files, infer_literal_type,
    parse_int_literal, promote_record_ty, register_ty, ty_to_match_ty, ty_to_match_ty_with_subst,
    vector_ty,
};
/// Type coercion — implicit type conversions (subtype coercions).
pub(super) mod coerce;
/// Existential type inference — skolemization and witness extraction.
pub mod existential;
/// Bidirectional mapping type checking (Sail-specific).
pub mod mapping;
mod numeric;
pub(super) mod nexp_simplify;
#[allow(dead_code)] // infrastructure — wired in A-5.2
pub(super) mod subtype;
/// Type well-formedness checking.
pub mod wf;
use numeric::*;
pub use numeric::{DEF_CACHE_HITS, DEF_CACHE_MISSES};
/// Expression type inference — walks expression tree and infers types.
mod expr;
/// Pattern type inference — bind_pattern_hir and helpers.
///
/// Adds `impl InferenceContext` methods for pattern inference.
mod pat;
pub(crate) use expr::InferenceContext;
mod workspace;
pub use workspace::*;

/// Span key used by the legacy checker for type tracking.
/// Will be removed when all callers migrate to InferenceResult.type_of_expr.
pub(super) type SpanKey = (usize, usize);

/// Nullary/unary constructors from the Sail standard library
/// (`sail/lib/option.sail`, `sail/lib/result.sail`). We whitelist them so
/// pattern-binding disambiguation and the unresolved-identifier check
/// don't misclassify legitimate uses just because the prelude
/// isn't in the parsed corpus.
const PRELUDE_CONSTRUCTORS: &[&str] = &["None", "Some", "Ok", "Err"];

/// Compiler intrinsics that parse as bare `Expr::Ident` but are always
/// resolved by the frontend.
const PRELUDE_INTRINSICS: &[&str] = &["__FILE__", "__LINE__"];

/// Enum members defined in Sail library files that projects
/// rely on without explicitly including them. Currently just the
/// concurrency interface (`sail/lib/concurrency_interface/read_write_v1.sail`),
/// which sail-riscv references via its `phys_mem_interface.sail`.
const PRELUDE_ENUM_MEMBERS: &[&str] = &[
    // enum Access_variety
    "AV_plain",
    "AV_exclusive",
    "AV_atomic_rmw",
    // enum Access_strength
    "AS_normal",
    "AS_rel_or_acq",
    "AS_acq_rcpc",
];

/// Names that should count as defined for the workspace-aware
/// unresolved-identifier check.
fn is_prelude_value(name: &str) -> bool {
    PRELUDE_CONSTRUCTORS.contains(&name)
        || PRELUDE_INTRINSICS.contains(&name)
        || PRELUDE_ENUM_MEMBERS.contains(&name)
}

// Type representation moved to `crate::ty` (top-level module).
// Re-export here for internal `infer/` access without changing every `use` line.
pub use crate::ty::{CompareOp, ConstraintExpr, Kind, NumericExpr, Scalar, Ty, TyArg, TyKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypeScheme {
    pub(super) quantifiers: Vec<String>,
    pub(super) kind_bounds: HashMap<String, Kind>,
    pub(super) constraints: Vec<QuantConstraint>,
    pub(super) params: Vec<Ty>,
    pub(super) implicit_params: Vec<bool>,
    pub(super) ret: Ty,
    pub(super) declared_effects: Vec<String>,
    pub(super) is_declared_pure: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MappingScheme {
    pub(super) quantifiers: Vec<String>,
    pub(super) constraints: Vec<QuantConstraint>,
    pub(super) lhs: Ty,
    pub(super) rhs: Ty,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LocalEnv {
    bindings: HashMap<String, Ty>,
    expected_return: Option<Ty>,
    constraints: Vec<ConstraintExpr>,
    undo_log: Vec<LocalEnvUndo>,
}

#[derive(Clone, Debug)]
enum LocalEnvUndo {
    PushScope,
    Define { name: String, previous: Option<Ty> },
    AddConstraint,
}

/// Positional generic arguments for instantiating type schemes.
#[derive(Clone, Debug, Default)]
pub(super) struct GenericArgs(pub SmallVec<[GenericArg; 4]>);

/// A single generic argument: either a type or a numeric value.
#[derive(Clone, Debug)]
pub(super) enum GenericArg {
    Type(Ty),
    Value(String),
}

impl GenericArgs {
    /// Build a Subst from quantifier names + GenericArgs (positional → named bridge).
    /// This allows gradual migration from Subst to GenericArgs.
    pub(super) fn to_subst(&self, quantifiers: &[String]) -> Subst {
        let mut subst = Subst::default();
        for (quant, arg) in quantifiers.iter().zip(self.0.iter()) {
            match arg {
                GenericArg::Type(ty) => {
                    subst.types.insert(quant.clone(), ty.clone());
                    subst.values.insert(quant.clone(), ty.display_text());
                }
                GenericArg::Value(val) => {
                    subst.values.insert(quant.clone(), val.clone());
                }
            }
        }
        subst
    }

    /// Build GenericArgs from TyArg slice (e.g., from TyKind::App args).
    pub(super) fn from_ty_args(args: &[TyArg]) -> Self {
        GenericArgs(
            args.iter()
                .map(|a| match a {
                    TyArg::Type(t) => GenericArg::Type(t.clone()),
                    TyArg::Nexp(n) => GenericArg::Value(n.to_string_repr()),
                    TyArg::Value(v) => GenericArg::Value(v.clone()),
                })
                .collect(),
        )
    }
}

/// Substitution maps generated by `unify` and consumed by `apply_subst`.
///
/// Backed by `SmallVec<[_; 8]>` (Sail subs are typically <= 4 entries).
#[derive(Clone, Debug, Default)]
pub struct Subst {
    pub types: SubstTypes,
    pub values: SubstValues,
}

#[derive(Clone, Debug, Default)]
pub struct SubstTypes {
    pub entries: SmallVec<[(String, Ty); 8]>,
}

impl SubstTypes {
    fn insert(&mut self, key: String, value: Ty) -> Option<Ty> {
        for entry in &mut self.entries {
            if entry.0 == key {
                let old = std::mem::replace(&mut entry.1, value);
                return Some(old);
            }
        }
        self.entries.push((key, value));
        None
    }

    fn get(&self, key: &str) -> Option<&Ty> {
        self.entries.iter().find_map(|(k, v)| if k == key { Some(v) } else { None })
    }

    fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SubstValues {
    pub entries: SmallVec<[(String, String); 8]>,
}

impl SubstValues {
    fn insert(&mut self, key: String, value: String) -> Option<String> {
        for entry in &mut self.entries {
            if entry.0 == key {
                let old = std::mem::replace(&mut entry.1, value);
                return Some(old);
            }
        }
        self.entries.push((key, value));
        None
    }

    fn get(&self, key: &str) -> Option<&String> {
        self.entries.iter().find_map(|(k, v)| if k == key { Some(v) } else { None })
    }

    fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuantConstraint {
    pub(super) text: String,
    mentions: Vec<String>,
    expr: ConstraintExpr,
}

// ConstraintExpr, CompareOp, NumericExpr moved to crate::ty.
// Re-exported at the top of this module via `pub use crate::ty::*`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintStatus {
    Satisfied,
    Failed,
    Unknown,
}

/// A deferred constraint obligation for batch solving.
#[derive(Clone, Debug)]
#[allow(dead_code)] // infrastructure — wired in A-3.4
pub enum Obligation {
    /// A numeric constraint that must hold (e.g., `'n + 'm < 64`).
    NumericConstraint(ConstraintExpr),
    /// Two numeric expressions that must be equal (e.g., bitvector
    /// widths in `bits('n)` operations).
    WidthEquality(NumericExpr, NumericExpr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NumericBound {
    pub(super) value: i64,
    pub(super) inclusive: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ConstraintFacts {
    pub(super) lower: Option<NumericBound>,
    pub(super) upper: Option<NumericBound>,
    pub(super) exact_values: Option<HashSet<i64>>,
    pub(super) excluded_values: HashSet<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArithmeticOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Backward-compatible alias: `TypeCheckResult` has been merged into `InferenceResult`.
pub type TypeCheckResult = InferenceResult;

//
// When inferring an expression, we propagate downward whatever type hint
// we have in the form of an `Expectation`. This replaces the old dual
// `infer_expr` (no hint) / `check_expr` (with hint) split.

/// Downward type hint for expression inference.
#[derive(Clone, Debug)]
pub(super) enum Expectation {
    /// No type hint.
    None,
    /// The expression should have exactly this type.
    HasType(Ty),
}

//
// Extracted to `infer/unify.rs` ().
// Re-exported here for backward compatibility.
pub mod unify;
pub use unify::{InferenceTable, InferenceTableSnapshot};

/// Per-callable inference result, keyed by `ExprId`/`PatId`.
#[derive(Clone, Debug, Default)]
pub struct InferenceResult {
    pub type_of_expr: ArenaMap<hir_def::ExprId, Ty>,
    pub type_of_pat: ArenaMap<hir_def::PatId, Ty>,
    pub type_of_binding: ArenaMap<hir_def::BindingId, Ty>,
    pub method_resolutions: FxHashMap<hir_def::ExprId, (hir_def::item_id::FunctionId, base_db::FileId)>,
    pub field_resolutions: FxHashMap<hir_def::ExprId, hir_def::ModuleDefId>,
    pub variant_resolutions: FxHashMap<hir_def::ExprOrPatId, hir_def::ModuleDefId>,
    pub expr_adjustments: FxHashMap<hir_def::ExprId, Vec<Adjustment>>,
    pub type_mismatches: FxHashMap<hir_def::ExprOrPatId, TypeMismatch>,
    pub has_errors: bool,
    pub diagnostics: Vec<InferenceDiagnostic>,
    pub unsolved_constraints: Vec<Obligation>,
    pub legacy_diagnostics: Vec<Diagnostic>,
    pub legacy_diagnostic_byte_spans: Vec<(usize, usize)>,
    pub bodies: Option<Arc<hir_def::bodies::CallableBodies>>,
    pub(crate) expr_types: HashMap<SpanKey, String>,
    pub(crate) binding_types: HashMap<SpanKey, String>,
}

/// A type adjustment (coercion) applied to an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adjustment {
    pub kind: Adjust,
    pub target: Ty,
}

/// The kind of type adjustment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Adjust {
    /// Numeric subtype coercion (nat → int, atom(N) → int).
    NumericSubtype,
    /// Range subsumption (range(lo1,hi1) → range(lo2,hi2)).
    RangeSubsume,
    /// Bitvector width equality check.
    BitvectorWidthEqual,
    /// Abstract kind coercion.
    AbstractKind,
    /// Never type to any type (diverging expression used in non-diverging context).
    NeverToAny,
}

/// A mismatch between an expected and an inferred type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMismatch {
    pub expected: Ty,
    pub actual: Ty,
}

/// Typed inference diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceDiagnostic {
    UnresolvedIdent { expr: hir_def::ExprId, name: String },
    UnresolvedField {
        expr: hir_def::ExprId,
        receiver: Ty,
        name: String,
        method_with_same_name_exists: bool,
    },
    MismatchedArgCount { call_expr: hir_def::ExprId, expected: usize, found: usize },
    ExpectedFunction { call_expr: hir_def::ExprId, found: Ty },
    EffectViolation { expr: hir_def::ExprId, effect: hir_def::EffectTag, context: &'static str },
    UnsolvedConstraint { expr: hir_def::ExprId, constraint: String },
    IncompleteMatch { expr: hir_def::ExprId, missing_arms: Vec<String> },
    MissingFields { expr: hir_def::ExprId, record_name: String, missing: Vec<String> },
    UnusedVariable { pat: hir_def::PatId, name: String },
    RemoveTrailingReturn { return_expr: hir_def::ExprId },
    RemoveUnnecessaryElse { if_expr: hir_def::ExprId },
    ConcatTypeMismatch { expr: hir_def::ExprId, message: String },
    ConstraintViolation {
        expr: hir_def::ExprId,
        constraint: String,
        derived_from: Vec<parser::Span>,
    },
    CallConstraintViolation {
        call_expr: hir_def::ExprId,
        constraint: String,
        derived_from: Vec<parser::Span>,
    },
    UnresolvedCallQuantifiers {
        call_expr: hir_def::ExprId,
        id: String,
        quants: Vec<String>,
        signature: Option<String>,
    },
    NoOverloading { call_expr: hir_def::ExprId, name: String },
    MappingBindingMismatch { expr: hir_def::ExprId, name: String, side: &'static str },
    DuplicateBinding { pat: hir_def::PatId, name: String },
    MissingPatternFields { pat: hir_def::PatId, record_name: String, missing: Vec<String> },
    NonContiguousSubrange { pat: hir_def::PatId },
    VectorSubrangeOrder {
        expr: hir_def::ExprId,
        first: String,
        second: String,
        order: hir_def::type_error::VectorOrder,
    },
    IncorrectCase {
        ident_expr: hir_def::ExprId,
        ident_type: &'static str,
        ident_text: String,
        expected_case: CaseType,
        suggested_text: String,
    },
}

/// Naming convention case types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaseType {
    /// snake_case (for functions, variables).
    LowerSnakeCase,
    /// CamelCase (for types, enum variants).
    UpperCamelCase,
}

/// Divergence tracking during inference.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Diverges {
    /// May or may not diverge.
    Maybe,
    /// Definitely diverges (e.g., `exit()`, `throw`, infinite loop).
    Always,
}

impl Diverges {
    pub fn is_always(self) -> bool {
        self == Diverges::Always
    }
}

impl std::ops::BitAnd for Diverges {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        std::cmp::min(self, other)
    }
}

impl std::ops::BitOr for Diverges {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        std::cmp::max(self, other)
    }
}

impl std::ops::BitAndAssign for Diverges {
    fn bitand_assign(&mut self, other: Self) {
        *self = *self & other;
    }
}

impl std::ops::BitOrAssign for Diverges {
    fn bitor_assign(&mut self, other: Self) {
        *self = *self | other;
    }
}

impl InferenceResult {
    // ── Methods merged from the former `TypeCheckResult` impl ────────

    /// Legacy text-based diagnostics accessor (was `TypeCheckResult::diagnostics`).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.legacy_diagnostics
    }

    /// On-demand span->type-text lookup. Walks InferenceResult's ExprId->Ty map
    /// and matches by span via Body's iter_exprs.
    ///
    /// Uses `self.bodies` (the same Bodies used during type checking) to ensure
    /// ExprId consistency. Falls back to the externally-provided `bodies` param.
    pub fn expr_type_text(
        &self,
        span: Span,
        bodies: Option<&hir_def::CallableBodies>,
    ) -> Option<String> {
        // First try ExprId->Ty path
        if let Some(bodies) = self.bodies.as_deref().or(bodies) {
            for entry in bodies.entries() {
                for (expr_id, _) in entry.body.iter_exprs() {
                    if let Some(s) = entry.source_map.expr_syntax(expr_id) {
                        if s.start == span.start && (s.end == span.end || s.end == span.end + 1) {
                            if let Some(ty) = self.expr_ty(expr_id) {
                                return Some(ty.display_text());
                            }
                        }
                    }
                }
            }
        }
        // Fallback: legacy span-based type map
        self.expr_types.get(&(span.start, span.end)).cloned()
    }

    /// On-demand span->binding-type-text lookup.
    pub fn binding_type_text(
        &self,
        span: Span,
        bodies: Option<&hir_def::CallableBodies>,
    ) -> Option<String> {
        // First try PatId->Ty path
        if let Some(bodies) = self.bodies.as_deref().or(bodies) {
            for entry in bodies.entries() {
                for (pat_id, _) in entry.body.iter_pats() {
                    if let Some(s) = entry.source_map.pat_syntax(pat_id) {
                        if s.start == span.start && s.end == span.end {
                            if let Some(ty) = self.pat_ty(pat_id) {
                                return Some(ty.display_text());
                            }
                        }
                    }
                }
            }
        }
        // Fallback: legacy span-based binding type map
        self.binding_types.get(&(span.start, span.end)).cloned()
    }

    // ── Original InferenceResult methods ─────────────────────────────

    /// Record the inferred type for an expression.
    pub fn write_expr_ty(&mut self, id: hir_def::ExprId, ty: Ty) {
        self.type_of_expr.insert(id, ty);
    }

    /// Record the inferred type for a pattern.
    pub fn write_pat_ty(&mut self, id: hir_def::PatId, ty: Ty) {
        self.type_of_pat.insert(id, ty);
    }

    pub fn record_method_resolution(
        &mut self,
        call_expr: hir_def::ExprId,
        func_id: hir_def::item_id::FunctionId,
        file_id: base_db::FileId,
    ) {
        self.method_resolutions.insert(call_expr, (func_id, file_id));
    }

    /// Record a type mismatch.
    /// Record a type mismatch for an expression.
    ///
    /// Convenience wrapper — converts ExprId to ExprOrPatId.
    pub fn record_type_mismatch(&mut self, expr_id: hir_def::ExprId, expected: &Ty, actual: &Ty) {
        self.record_type_mismatch_at(hir_def::ExprOrPatId::ExprId(expr_id), expected, actual);
    }

    /// Record a type mismatch at any expression or pattern location.
    pub fn record_type_mismatch_at(
        &mut self,
        id: hir_def::ExprOrPatId,
        expected: &Ty,
        actual: &Ty,
    ) {
        // Skip recording if either type contains Error.
        // This prevents cascading diagnostics from a single root cause.
        if expected.is_error() || actual.is_error() {
            return;
        }
        self.type_mismatches
            .insert(id, TypeMismatch { expected: expected.clone(), actual: actual.clone() });
    }

    /// Record which field a field-access expression resolves to.
    ///
    /// Enables precise goto-def for field accesses.
    pub fn record_field_resolution(
        &mut self,
        expr_id: hir_def::ExprId,
        field_def: hir_def::ModuleDefId,
    ) {
        self.field_resolutions.insert(expr_id, field_def);
    }

    /// Record which variant/constructor an expression or pattern resolves to.
    pub fn record_variant_resolution(
        &mut self,
        id: impl Into<hir_def::ExprOrPatId>,
        variant_def: hir_def::ModuleDefId,
    ) {
        self.variant_resolutions.insert(id.into(), variant_def);
    }

    /// Look up the type of an expression.
    pub fn expr_ty(&self, id: hir_def::ExprId) -> Option<&Ty> {
        self.type_of_expr.get(id)
    }

    /// Look up the type of a pattern.
    pub fn pat_ty(&self, id: hir_def::PatId) -> Option<&Ty> {
        self.type_of_pat.get(id)
    }

    /// Look up a type mismatch for an expression.
    pub fn type_mismatch_for_expr(&self, expr: hir_def::ExprId) -> Option<&TypeMismatch> {
        self.type_mismatches.get(&hir_def::ExprOrPatId::ExprId(expr))
    }

    /// Look up a type mismatch for a pattern.
    pub fn type_mismatch_for_pat(&self, pat: hir_def::PatId) -> Option<&TypeMismatch> {
        self.type_mismatches.get(&hir_def::ExprOrPatId::PatId(pat))
    }

    /// Iterate all type mismatches.
    pub fn type_mismatches_iter(
        &self,
    ) -> impl Iterator<Item = (hir_def::ExprOrPatId, &TypeMismatch)> {
        self.type_mismatches.iter().map(|(&id, m)| (id, m))
    }

    /// Iterate only expression-level type mismatches (backward-compat).
    pub fn expr_type_mismatches_iter(
        &self,
    ) -> impl Iterator<Item = (hir_def::ExprId, &TypeMismatch)> + '_ {
        self.type_mismatches.iter().filter_map(|(&id, m)| match id {
            hir_def::ExprOrPatId::ExprId(expr_id) => Some((expr_id, m)),
            hir_def::ExprOrPatId::PatId(_) => None,
        })
    }

    /// Iterate all expression types.
    pub fn expression_types(&self) -> impl Iterator<Item = (hir_def::ExprId, &Ty)> + '_ {
        self.type_of_expr.iter()
    }

    /// Iterate all pattern types.
    pub fn pattern_types(&self) -> impl Iterator<Item = (hir_def::PatId, &Ty)> + '_ {
        self.type_of_pat.iter()
    }

    /// Access typed diagnostics.
    pub fn inference_diagnostics(&self) -> &[InferenceDiagnostic] {
        &self.diagnostics
    }

    /// Convert to TypeCheckResult (now just a clone since they are the same type).
    pub fn to_type_check_result(
        &self,
        _body: &hir_def::Body,
        _source_map: &hir_def::BodySourceMap,
    ) -> TypeCheckResult {
        self.clone()
    }

    /// Merge another InferenceResult into this one (for per-file aggregation).
    pub fn merge(&mut self, other: InferenceResult) {
        for (id, ty) in other.type_of_expr.iter() {
            self.type_of_expr.insert(id, ty.clone());
        }
        for (id, ty) in other.type_of_pat.iter() {
            self.type_of_pat.insert(id, ty.clone());
        }
        self.type_mismatches.extend(other.type_mismatches);
        self.has_errors |= other.has_errors;
        self.diagnostics.extend(other.diagnostics);
        self.method_resolutions.extend(other.method_resolutions);
        self.unsolved_constraints.extend(other.unsolved_constraints);
        self.legacy_diagnostics.extend(other.legacy_diagnostics);
        self.legacy_diagnostic_byte_spans.extend(other.legacy_diagnostic_byte_spans);
        self.expr_types.extend(other.expr_types);
        self.binding_types.extend(other.binding_types);
        // bodies: keep self's if present, otherwise take other's
        if self.bodies.is_none() {
            self.bodies = other.bodies;
        }
    }

    /// Number of expressions with inferred types.
    pub fn expr_count(&self) -> usize {
        self.type_of_expr.iter().count()
    }
}

impl Ty {
    /// Render this type as a textual form for diagnostics.
    pub fn display_text(&self) -> String {
        let mut out = String::new();
        self.display_text_into(&mut out);
        out
    }

    fn display_text_into(&self, out: &mut String) {
        match self.kind() {
            TyKind::Error => out.push('_'),
            TyKind::Infer(crate::ty::InferTy(id)) => {
                out.push_str(&format!("?{id}"));
            }
            TyKind::Scalar(s) => out.push_str(s.name()),
            TyKind::Adt(text, _) => out.push_str(text),
            TyKind::Param(name) => out.push_str(name),
            TyKind::Tuple(items) => {
                out.push('(');
                let mut first = true;
                for item in items.iter() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    item.display_text_into(out);
                }
                out.push(')');
            }
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                if params.len() == 1 {
                    params[0].display_text_into(out);
                } else {
                    out.push('(');
                    let mut first = true;
                    for param in params.iter() {
                        if !first {
                            out.push_str(", ");
                        }
                        first = false;
                        param.display_text_into(out);
                    }
                    out.push(')');
                }
                out.push_str(" -> ");
                ret.display_text_into(out);
            }
            TyKind::App { text, .. } => out.push_str(text),
            TyKind::Exist { vars, constraint, inner } => {
                out.push('{');
                out.push_str(&vars.join(", "));
                let c_text = format!("{constraint:?}");
                if c_text != "Bool(true)" && c_text != "Unsupported" {
                    out.push_str(", ");
                    out.push_str(&c_text);
                }
                out.push_str(". ");
                inner.display_text_into(out);
                out.push('}');
            }
            TyKind::Bidir { lhs, rhs } => {
                lhs.display_text_into(out);
                out.push_str(" <-> ");
                rhs.display_text_into(out);
            }
            TyKind::Abstract { name, kind } => {
                out.push_str(name);
                out.push_str(" : ");
                out.push_str(&format!("{:?}", kind));
            }
        }
    }

    pub(super) fn is_unknown(&self) -> bool {
        matches!(self.kind(), TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)))
    }
}

impl LocalEnv {
    pub(super) fn new(expected_return: Option<Ty>) -> Self {
        Self {
            bindings: HashMap::new(),
            expected_return,
            constraints: Vec::new(),
            undo_log: Vec::new(),
        }
    }

    fn restore(&mut self, mark: usize) {
        while self.undo_log.len() > mark {
            match self.undo_log.pop().expect("undo log length checked") {
                LocalEnvUndo::PushScope => {}
                LocalEnvUndo::Define { name, previous } => {
                    if let Some(previous) = previous {
                        self.bindings.insert(name, previous);
                    } else {
                        self.bindings.remove(&name);
                    }
                }
                LocalEnvUndo::AddConstraint => {
                    self.constraints.pop();
                }
            }
        }
    }

    fn push_scope(&mut self) {
        self.undo_log.push(LocalEnvUndo::PushScope);
    }

    fn pop_scope(&mut self) {
        if let Some(mark) =
            self.undo_log.iter().rposition(|entry| matches!(entry, LocalEnvUndo::PushScope))
        {
            self.restore(mark);
        }
    }

    fn define(&mut self, name: &str, ty: Ty) {
        let name = name.to_string();
        let previous = self.bindings.insert(name.clone(), ty);
        self.undo_log.push(LocalEnvUndo::Define { name, previous });
    }

    fn add_constraint(&mut self, constraint: ConstraintExpr) {
        self.constraints.push(constraint);
        self.undo_log.push(LocalEnvUndo::AddConstraint);
    }

    fn lookup(&self, name: &str) -> Option<&Ty> {
        self.bindings.get(name)
    }
}

/// Extract a `Ty` from a rowan CST type-expression node.
/// CST-native equivalent of `type_from_type_expr`.
pub(crate) fn type_from_cst_node(node: &syntax::SyntaxNode) -> Ty {
    use parser::SyntaxKind as SK;

    match node.kind() {
        SK::TYPE_NAMED => {
            let text =
                cst_ident_text(node).unwrap_or_else(|| node.text().to_string().trim().to_string());
            Ty::named(text)
        }
        SK::TYPE_VAR => {
            let text = node.text().to_string().trim().to_string();
            Ty::param(text)
        }
        SK::TYPE_TUPLE => {
            let items: Vec<Ty> = node.children().map(|c| type_from_cst_node(&c)).collect();
            if items.len() == 1 {
                items.into_iter().next().unwrap()
            } else {
                Ty::tuple(items)
            }
        }
        SK::TYPE_ARROW => {
            // Distinguish `->` (function) from `<->` (bidir mapping)
            let is_bidir = node
                .children_with_tokens()
                .any(|el| el.as_token().map_or(false, |t| t.kind() == SK::DOUBLE_ARROW));
            let children: Vec<_> = node.children().collect();
            if children.len() >= 2 {
                let lhs_node = &children[0];
                let rhs_node = &children[children.len() - 1];
                if is_bidir {
                    let lhs = type_from_cst_node(lhs_node);
                    let rhs = type_from_cst_node(rhs_node);
                    Ty::bidir(lhs, rhs)
                } else {
                    let params = match lhs_node.kind() {
                        SK::TYPE_TUPLE => {
                            lhs_node.children().map(|c| type_from_cst_node(&c)).collect()
                        }
                        _ => vec![type_from_cst_node(lhs_node)],
                    };
                    let ret = type_from_cst_node(rhs_node);
                    Ty::function(params, ret)
                }
            } else if let Some(child) = children.first() {
                type_from_cst_node(child)
            } else {
                Ty::error()
            }
        }
        SK::TYPE_APP => {
            // Normal: first child node = name (TYPE_NAMED with IDENT),
            // rest = args. For keyword types (register), name comes from
            // a token, and ALL child nodes are args.
            let children: Vec<_> = node.children().collect();
            let name_from_child = children.first().and_then(|c| cst_ident_text(c));
            let (name, args_start) = if let Some(n) = name_from_child {
                (n, 1) // skip first child (it's the name)
            } else {
                // Keyword type: name from token, all children are args
                let kw_name = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| matches!(t.kind(), SK::KW_REGISTER))
                    .map(|t| t.text().to_string())
                    .unwrap_or_else(|| {
                        children
                            .first()
                            .map(|c| c.text().to_string().trim().to_string())
                            .unwrap_or_default()
                    });
                (kw_name, 0) // don't skip — all children are args
            };
            let args: Vec<TyArg> =
                children.iter().skip(args_start).map(|c| type_arg_from_cst_node(c)).collect();
            let text = node.text().to_string().trim().to_string();
            Ty::app(name, args, text)
        }
        SK::TYPE_FORALL => {
            // Walk to inner body type (last child that's a type node)
            if let Some(body) = node.children().last() {
                type_from_cst_node(&body)
            } else {
                Ty::error()
            }
        }
        SK::TYPE_EXISTENTIAL => {
            // Extract existential quantifiers, constraints, and body.
            // CST structure: {<vars>, <constraint>. <body_type>} or exist <vars>. <body_type>
            let mut vars = Vec::new();
            let mut constraint = ConstraintExpr::Bool(true);

            // Collect tokens before `.` for vars and constraints
            let mut all_tokens: Vec<(SK, String)> = Vec::new();
            for tok_or_node in node.children_with_tokens() {
                if let Some(tok) = tok_or_node.as_token() {
                    if tok.kind() == SK::DOT {
                        break;
                    }
                    if !tok.kind().is_trivia() {
                        all_tokens.push((tok.kind(), tok.text().to_string()));
                    }
                }
            }

            // Extract type vars
            let constraint_start = all_tokens.iter().position(|(k, text)| {
                matches!(k, SK::L_ANGLE | SK::R_ANGLE | SK::LE | SK::GE | SK::EQ_EQ) || text == "in"
            });
            let quant_end = constraint_start.unwrap_or(all_tokens.len());
            for (k, text) in &all_tokens[..quant_end] {
                if *k == SK::TY_VAR && !vars.contains(text) {
                    vars.push(text.clone());
                }
            }

            // Parse constraint
            if let Some(start) = constraint_start {
                let comma_before = all_tokens[..start]
                    .iter()
                    .rposition(|(k, _)| *k == SK::COMMA)
                    .map(|i| i + 1)
                    .unwrap_or(start);
                let constraint_tokens = &all_tokens[comma_before..];
                constraint = parse_constraint_from_tokens(constraint_tokens);
            }

            // Parse body type
            let inner = node
                .children()
                .last()
                .map(|body| type_from_cst_node(&body))
                .unwrap_or_else(Ty::error);

            Ty::exist(vars, constraint, inner)
        }
        SK::TYPE_EFFECT => {
            // Effect wraps an inner type
            if let Some(body) = node.children().last() {
                type_from_cst_node(&body)
            } else {
                Ty::error()
            }
        }
        _ => {
            // Fallback: use the node text as a text type
            let text = node.text().to_string().trim().to_string();
            if text.is_empty() {
                Ty::error()
            } else {
                Ty::named(text)
            }
        }
    }
}

fn type_arg_from_cst_node(node: &syntax::SyntaxNode) -> TyArg {
    use parser::SyntaxKind as SK;
    match node.kind() {
        SK::TYPE_NAMED => {
            // Distinguish Value args (numeric: "8", "64", "8 * 'n",
            // "'n + 1", "-1") from Type args ("int", "bool", "dec").
            let text = node.text().to_string();
            let trimmed = text.trim();
            let first = trimmed.chars().next().unwrap_or(' ');
            let is_value =
                first.is_ascii_digit() || (first == '-' && trimmed.len() > 1) || first == '(';
            if is_value {
                TyArg::numeric(trimmed.to_string())
            } else {
                TyArg::Type(type_from_cst_node(node))
            }
        }
        SK::TYPE_VAR => {
            // Type variables like `'a` in args position could be either
            // Type or Value depending on context. Default to Type
            // (polymorphic type arg). Numeric expressions like `'n + 1`
            // are absorbed into TYPE_NAMED by absorb_numeric_infix.
            TyArg::Type(type_from_cst_node(node))
        }
        SK::TYPE_APP
        | SK::TYPE_TUPLE
        | SK::TYPE_ARROW
        | SK::TYPE_FORALL
        | SK::TYPE_EXISTENTIAL
        | SK::TYPE_EFFECT => TyArg::Type(type_from_cst_node(node)),
        _ => TyArg::numeric(node.text().to_string().trim().to_string()),
    }
}

/// Extract a `TypeScheme` from a CST type-expression node (typically
/// the signature portion of a val spec or function definition).
/// CST-native equivalent of `scheme_from_type_expr`.
fn ty_is_implicit(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::App { name, .. } if name == "implicit")
}

/// Extract effect names from a TYPE_EFFECT CST node.
///
/// The CST structure is `effect { ident1 , ident2 , ... }`.
/// Walks the full type tree to find any TYPE_EFFECT descendant
/// and extracts IDENT tokens between `{` and `}`.
fn extract_effects_from_cst_type(node: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    let mut effects = Vec::new();
    for desc in node.descendants() {
        if ast::TypeEffect::can_cast(desc.kind()) {
            for token in desc.descendants_with_tokens().filter_map(|el| el.into_token()) {
                if token.kind() == SK::IDENT {
                    effects.push(token.text().to_string());
                }
            }
        }
    }
    effects
}

pub(crate) fn scheme_from_cst_node(node: &syntax::SyntaxNode) -> TypeScheme {
    let mut quantifiers = Vec::new();
    let mut constraints = Vec::new();
    let declared_effects = extract_effects_from_cst_type(node);
    // Detect explicit `pure` keyword in val spec.
    let is_declared_pure = node
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| t.kind() == parser::SyntaxKind::KW_PURE);

    // Collect forall quantifiers and constraints
    let body_node = collect_forall_from_cst(node, &mut quantifiers, &mut constraints);
    let inner = body_node.as_ref().unwrap_or(node);

    let parsed = type_from_cst_node(inner);
    match parsed.kind() {
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
            let ret = ret.clone();
            let (params, implicit_params) = if params.len() == 1 {
                if let TyKind::Tuple(items) = params[0].kind() {
                    let implicit = items.iter().map(ty_is_implicit).collect();
                    (items.clone(), implicit)
                } else {
                    let implicit = params.iter().map(ty_is_implicit).collect();
                    (params.clone(), implicit)
                }
            } else {
                let implicit = params.iter().map(ty_is_implicit).collect();
                (params.clone(), implicit)
            };
            TypeScheme {
                quantifiers,
                kind_bounds: HashMap::new(),
                constraints,
                params,
                implicit_params,
                ret,
                declared_effects,
                is_declared_pure,
            }
        }
        _ => TypeScheme {
            quantifiers,
            kind_bounds: HashMap::new(),
            constraints,
            params: Vec::new(),
            implicit_params: Vec::new(),
            ret: parsed,
            declared_effects,
            is_declared_pure,
        },
    }
}

/// Walk a CST type node to collect forall quantifiers. Returns the
/// inner body node (after stripping forall wrappers).
fn collect_forall_from_cst(
    node: &syntax::SyntaxNode,
    quantifiers: &mut Vec<String>,
    _constraints: &mut Vec<QuantConstraint>,
) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;

    if !ast::TypeForall::can_cast(node.kind()) {
        return None;
    }

    // Collect tokens between `forall` and `.`.
    // TY_VAR tokens are quantifier names.
    // Everything after the last TY_VAR (or `:Kind`) that looks like a
    // constraint expression is collected as constraint text.
    let mut all_tokens: Vec<(SK, String)> = Vec::new();
    for tok_or_node in node.children_with_tokens() {
        if let Some(tok) = tok_or_node.as_token() {
            if tok.kind() == SK::DOT {
                break;
            }
            if !tok.kind().is_trivia() {
                all_tokens.push((tok.kind(), tok.text().to_string()));
            }
        }
    }

    let constraint_start = all_tokens.iter().position(|(k, text)| {
        matches!(k, SK::L_ANGLE | SK::R_ANGLE | SK::LE | SK::GE | SK::EQ_EQ) || text == "in"
    });

    // Collect quantifier names only from tokens before the constraint.
    let quant_end = constraint_start
        .and_then(|s| all_tokens[..s].iter().rposition(|(k, _)| *k == SK::COMMA).map(|i| i))
        .unwrap_or(all_tokens.len());
    for (k, text) in &all_tokens[..quant_end] {
        if *k == SK::TY_VAR && !quantifiers.contains(text) {
            quantifiers.push(text.clone());
        }
    }

    if let Some(start) = constraint_start {
        let comma_before = all_tokens[..start]
            .iter()
            .rposition(|(k, _)| *k == SK::COMMA)
            .map(|i| i + 1)
            .unwrap_or(start);
        let raw_text: String = all_tokens[comma_before..]
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Normalize: `{ 1 , 2 }` → `{1, 2}`
        let constraint_text = raw_text.replace("{ ", "{").replace(" }", "}").replace(" , ", ", ");
        if !constraint_text.is_empty() {
            // Extract mentioned type variables
            let mut mentions: Vec<String> = all_tokens[comma_before..]
                .iter()
                .filter(|(k, _)| *k == SK::TY_VAR)
                .map(|(_, t)| t.clone())
                .collect();
            mentions.sort();
            mentions.dedup();
            let constraint_tokens = &all_tokens[comma_before..];
            let expr = parse_constraint_from_tokens(constraint_tokens);
            _constraints.push(QuantConstraint { text: constraint_text, mentions, expr });
        }
    }

    // Return the last child node (the body type)
    node.children().last()
}

/// Parse a constraint expression from forall preamble tokens.
///
/// Handles:
/// - `'n in { 1 , 2 }` → InSet
/// - `0 < 'n <= 64` → And([Compare(0 < 'n), Compare('n <= 64)])
/// - `'n >= 0` → Compare('n >= 0)
fn parse_constraint_from_tokens(tokens: &[(parser::SyntaxKind, String)]) -> ConstraintExpr {
    use parser::SyntaxKind as SK;

    // 0. Split at `|` (disjunction) — lowest precedence (below `&`).
    // Only split at top-level `|` tokens (not inside parens/braces).
    let pipe_positions: Vec<usize> = {
        let mut depth = 0i32;
        tokens
            .iter()
            .enumerate()
            .filter_map(|(i, (k, _))| {
                match k {
                    SK::L_PAREN | SK::L_CURLY => { depth += 1; None }
                    SK::R_PAREN | SK::R_CURLY => { depth -= 1; None }
                    SK::PIPE if depth == 0 => Some(i),
                    _ => None,
                }
            })
            .collect()
    };
    if !pipe_positions.is_empty() {
        let mut parts: Vec<ConstraintExpr> = Vec::new();
        let mut start = 0;
        for &pos in &pipe_positions {
            let sub = parse_constraint_from_tokens(&tokens[start..pos]);
            if !matches!(sub, ConstraintExpr::Unsupported) {
                parts.push(sub);
            }
            start = pos + 1;
        }
        let sub = parse_constraint_from_tokens(&tokens[start..]);
        if !matches!(sub, ConstraintExpr::Unsupported) {
            parts.push(sub);
        }
        if parts.len() > 1 {
            return ConstraintExpr::Or(parts);
        } else if parts.len() == 1 {
            return parts.into_iter().next().unwrap();
        }
    }

    // 1. Split at `&` (conjunction) — lower precedence than atoms, higher than `|`.
    // Only split at top-level `&` tokens (not inside parens/braces).
    let amp_positions: Vec<usize> = {
        let mut depth = 0i32;
        tokens
            .iter()
            .enumerate()
            .filter_map(|(i, (k, _))| {
                match k {
                    SK::L_PAREN | SK::L_CURLY => { depth += 1; None }
                    SK::R_PAREN | SK::R_CURLY => { depth -= 1; None }
                    SK::AMP if depth == 0 => Some(i),
                    _ => None,
                }
            })
            .collect()
    };
    if !amp_positions.is_empty() {
        let mut parts: Vec<ConstraintExpr> = Vec::new();
        let mut start = 0;
        for &pos in &amp_positions {
            let sub = parse_constraint_from_tokens(&tokens[start..pos]);
            if !matches!(sub, ConstraintExpr::Unsupported) {
                parts.push(sub);
            }
            start = pos + 1;
        }
        let sub = parse_constraint_from_tokens(&tokens[start..]);
        if !matches!(sub, ConstraintExpr::Unsupported) {
            parts.push(sub);
        }
        if parts.len() > 1 {
            return ConstraintExpr::And(parts);
        } else if parts.len() == 1 {
            return parts.into_iter().next().unwrap();
        }
    }

    // 2. Check for bare `true`/`false`.
    let meaningful: Vec<_> = tokens.iter().filter(|(k, _)| !k.is_trivia()).collect();
    if meaningful.len() == 1 {
        match meaningful[0].1.as_str() {
            "true" => return ConstraintExpr::Bool(true),
            "false" => return ConstraintExpr::Bool(false),
            _ => {}
        }
    }

    // 2b. Check for `not(...)` pattern.
    // Look for a leading `not` IDENT followed by `(` ... `)` spanning the rest.
    {
        let mf: Vec<_> = tokens.iter().filter(|(k, _)| !k.is_trivia()).collect();
        if mf.len() >= 3
            && mf[0].0 == SK::IDENT
            && mf[0].1 == "not"
            && mf[1].0 == SK::L_PAREN
            && mf[mf.len() - 1].0 == SK::R_PAREN
        {
            // Find the matching close paren for the opening paren after `not`.
            // We need raw token indices for slicing.
            let not_end = tokens.len();
            // Find index of first L_PAREN after `not` in raw tokens.
            if let Some(open_idx) = tokens
                .iter()
                .position(|(k, v)| !k.is_trivia() && *k == SK::IDENT && v == "not")
                .and_then(|not_raw| {
                    tokens[not_raw + 1..]
                        .iter()
                        .position(|(k, _)| *k == SK::L_PAREN)
                        .map(|rel| not_raw + 1 + rel)
                })
            {
                // Walk to find the matching close paren.
                let mut depth = 0i32;
                let mut close_idx = None;
                for (j, (k, _)) in tokens[open_idx..].iter().enumerate() {
                    match k {
                        SK::L_PAREN => depth += 1,
                        SK::R_PAREN => {
                            depth -= 1;
                            if depth == 0 {
                                close_idx = Some(open_idx + j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(close) = close_idx {
                    if close + 1 == not_end
                        || tokens[close + 1..].iter().all(|(k, _)| k.is_trivia())
                    {
                        let inner_tokens = &tokens[open_idx + 1..close];
                        let inner = parse_constraint_from_tokens(inner_tokens);
                        return ConstraintExpr::Not(Box::new(inner));
                    }
                }
            }
        }
    }

    // 3. Check for `X in { items }` pattern.
    if let Some(in_pos) = tokens.iter().position(|(_, t)| t == "in") {
        let lhs_tokens = &tokens[..in_pos];
        let rhs_tokens = &tokens[in_pos + 1..];

        if let Some(value) = numeric_expr_from_token_slice(lhs_tokens) {
            // Parse { N1, N2, ... }
            let items: Vec<NumericExpr> = rhs_tokens
                .iter()
                .filter(|(k, _)| matches!(k, SK::NUM_LIT | SK::TY_VAR | SK::IDENT))
                .filter(|(k, _)| *k != SK::L_CURLY && *k != SK::R_CURLY)
                .filter_map(|(k, t)| match k {
                    SK::NUM_LIT => t.parse::<i64>().ok().map(NumericExpr::Const),
                    SK::TY_VAR => Some(NumericExpr::Var(t.clone())),
                    SK::IDENT => Some(NumericExpr::Symbol(t.clone())),
                    _ => None,
                })
                .collect();
            if !items.is_empty() {
                return ConstraintExpr::InSet { value, items };
            }
        }
    }

    // 4. Check for chained comparison: `A op1 B op2 C` → And([Compare(A op1 B), Compare(B op2 C)])
    let cmp_positions: Vec<(usize, CompareOp)> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, (k, t))| match k {
            SK::L_ANGLE => Some((i, CompareOp::Lt)),
            SK::R_ANGLE => Some((i, CompareOp::Gt)),
            SK::LE => Some((i, CompareOp::Lte)),
            SK::GE => Some((i, CompareOp::Gte)),
            SK::EQ_EQ => Some((i, CompareOp::Eq)),
            _ if t == "!=" => Some((i, CompareOp::Neq)),
            _ => None,
        })
        .collect();

    if cmp_positions.len() >= 2 {
        // Chained: A op1 B op2 C → And([A op1 B, B op2 C])
        let mut parts = Vec::new();
        let mut prev_start = 0;
        for (idx, &(pos, op)) in cmp_positions.iter().enumerate() {
            let lhs_slice = &tokens[prev_start..pos];
            let next_end =
                if idx + 1 < cmp_positions.len() { cmp_positions[idx + 1].0 } else { tokens.len() };
            let rhs_slice = &tokens[pos + 1..next_end];
            if let (Some(lhs), Some(rhs)) =
                (numeric_expr_from_token_slice(lhs_slice), numeric_expr_from_token_slice(rhs_slice))
            {
                parts.push(ConstraintExpr::Compare { lhs, op, rhs });
            }
            prev_start = pos + 1;
        }
        if parts.len() > 1 {
            return ConstraintExpr::And(parts);
        } else if parts.len() == 1 {
            return parts.into_iter().next().unwrap();
        }
    } else if cmp_positions.len() == 1 {
        let (pos, op) = cmp_positions[0];
        let lhs_slice = &tokens[..pos];
        let rhs_slice = &tokens[pos + 1..];
        if let (Some(lhs), Some(rhs)) =
            (numeric_expr_from_token_slice(lhs_slice), numeric_expr_from_token_slice(rhs_slice))
        {
            return ConstraintExpr::Compare { lhs, op, rhs };
        }
    }

    ConstraintExpr::Unsupported
}

/// Parse a constraint expression from a plain text string.
///
/// Tokenizes the text using a simple character scanner and delegates to
/// `parse_constraint_from_tokens`. Used for `NumericExpr::If` conditions
/// which are parsed as text by `NumericTextParser`.
pub(super) fn parse_constraint_text(text: &str) -> ConstraintExpr {
    use parser::SyntaxKind as SK;

    let mut tokens: Vec<(SK, String)> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_ascii_whitespace() {
            tokens.push((SK::WHITESPACE, chars[i].to_string()));
            i += 1;
            continue;
        }
        // Type variable: starts with '\'
        if chars[i] == '\'' {
            let mut s = String::from('\'');
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push((SK::TY_VAR, s));
            continue;
        }
        // Number
        if chars[i].is_ascii_digit() || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let mut s = String::new();
            if chars[i] == '-' {
                s.push('-');
                i += 1;
            }
            while i < chars.len() && chars[i].is_ascii_digit() {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push((SK::NUM_LIT, s));
            continue;
        }
        // Identifiers / keywords
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut s = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push((SK::IDENT, s));
            continue;
        }
        // Two-char operators
        if i + 1 < chars.len() {
            let two: String = [chars[i], chars[i + 1]].iter().collect();
            match two.as_str() {
                "<=" => { tokens.push((SK::LE, two)); i += 2; continue; }
                ">=" => { tokens.push((SK::GE, two)); i += 2; continue; }
                "==" => { tokens.push((SK::EQ_EQ, two)); i += 2; continue; }
                "!=" => { tokens.push((SK::NEQ, "!=".to_string())); i += 2; continue; }
                _ => {}
            }
        }
        // Single-char tokens
        let kind = match chars[i] {
            '<' => SK::L_ANGLE,
            '>' => SK::R_ANGLE,
            '=' => SK::EQ,
            '&' => SK::AMP,
            '|' => SK::PIPE,
            '(' => SK::L_PAREN,
            ')' => SK::R_PAREN,
            '{' => SK::L_CURLY,
            '}' => SK::R_CURLY,
            ',' => SK::COMMA,
            '+' => SK::PLUS,
            '-' => SK::MINUS,
            '*' => SK::STAR,
            '/' => SK::SLASH,
            '%' => SK::PERCENT,
            '^' => SK::CARET,
            _ => SK::ERROR,
        };
        tokens.push((kind, chars[i].to_string()));
        i += 1;
    }

    parse_constraint_from_tokens(&tokens)
}

/// Parse a numeric expression from a slice of tokens.
fn numeric_expr_from_token_slice(tokens: &[(parser::SyntaxKind, String)]) -> Option<NumericExpr> {
    use parser::SyntaxKind as SK;
    // Filter to meaningful tokens
    let meaningful: Vec<_> = tokens
        .iter()
        .filter(|(k, _)| !k.is_trivia() && *k != SK::L_PAREN && *k != SK::R_PAREN)
        .collect();

    if meaningful.is_empty() {
        return None;
    }
    if meaningful.len() == 1 {
        let (k, t) = meaningful[0];
        return match k {
            SK::NUM_LIT => t.parse::<i64>().ok().map(NumericExpr::Const),
            SK::TY_VAR => Some(NumericExpr::Var(t.clone())),
            SK::IDENT => Some(NumericExpr::Symbol(t.clone())),
            _ => None,
        };
    }
    // Multi-token: try simple binary `A op B`
    if meaningful.len() == 3 {
        let lhs = numeric_expr_from_token_slice(&[(meaningful[0].0, meaningful[0].1.clone())]);
        let rhs = numeric_expr_from_token_slice(&[(meaningful[2].0, meaningful[2].1.clone())]);
        if let (Some(l), Some(r)) = (lhs, rhs) {
            let op_text = &meaningful[1].1;
            return match op_text.as_str() {
                "+" => Some(NumericExpr::Add(Box::new(l), Box::new(r))),
                "-" => Some(NumericExpr::Sub(Box::new(l), Box::new(r))),
                "*" => Some(NumericExpr::Mul(Box::new(l), Box::new(r))),
                "/" => Some(NumericExpr::Div(Box::new(l), Box::new(r))),
                // B2-2: Exponentiation (2^n). Sail convention: base is always 2.
                "^" => Some(NumericExpr::Exp(Box::new(r))),
                _ => None,
            };
        }
    }
    // Fallback: treat full text as a symbol
    let text: String = tokens
        .iter()
        .filter(|(k, _)| !k.is_trivia())
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        None
    } else {
        Some(NumericExpr::Symbol(text))
    }
}

/// Extract the definition name from a CST node.
/// Prefers the NAME child node (created by the parser for definitions);
/// falls back to the first IDENT token in direct children only (not
/// descendants, to avoid picking up identifiers from body expressions).
fn cst_ident_text(node: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    // First: look for a NAME child node
    for child in node.children() {
        if ast::Name::can_cast(child.kind()) {
            return child
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| t.text().to_string());
        }
    }
    // Fallback: first IDENT in direct children tokens, but stop at `=`
    // to avoid picking up identifiers from body expressions.
    for el in node.children_with_tokens() {
        if let Some(tok) = el.as_token() {
            if tok.kind() == SK::EQ {
                break;
            }
            if tok.kind() == SK::IDENT {
                return Some(tok.text().to_string());
            }
        }
    }
    None
}

/// Extract all IDENT tokens from a CST node descendants.
fn cst_ident_texts(node: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
        .collect()
}

/// For `overload operator | = {or_vec}`, extract the operator symbol
/// ("|", "&", "-", etc.) that appears between the `operator` IDENT and
/// the `=` token.  Returns `None` if no operator token is found.
fn extract_operator_overload_key(node: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    let mut saw_operator_ident = false;
    for el in node.descendants_with_tokens() {
        let tok = match el.into_token() {
            Some(t) => t,
            None => continue,
        };
        let k = tok.kind();
        if k.is_trivia() {
            continue;
        }
        if k == SK::EQ {
            break;
        }
        if k == SK::IDENT && tok.text() == "operator" {
            saw_operator_ident = true;
            continue;
        }
        if saw_operator_ident {
            // The next non-trivia, non-EQ token after "operator" is the
            // operator symbol (PIPE, AMP, MINUS, TILDE, etc.).
            return Some(tok.text().to_string());
        }
    }
    None
}

/// Extract typed params from a CALLABLE_DEF's PARAM_LIST.
/// Returns one Ty per TYPE_* child found inside PARAM_LIST.
fn extract_param_types_from_param_list(def_node: &syntax::SyntaxNode) -> Vec<Ty> {
    let Some(param_list) = def_node.children().find(|n| ast::ParamList::can_cast(n.kind())) else {
        return Vec::new();
    };
    // Collect all TYPE_* descendants inside the PARAM_LIST.
    // Each corresponds to one param's `: Type` annotation.
    param_list
        .descendants()
        .filter(|n| n != &param_list && n.kind().is_type_node())
        // Only take top-level type nodes (not nested children of other type nodes)
        .filter(|n| {
            // A top-level type node's parent is PARAM_LIST itself
            // (or a non-TYPE intermediate like BLOCK_ITEM).
            n.parent().map(|p| !p.kind().is_type_node()).unwrap_or(true)
        })
        .map(|n| type_from_cst_node(&n))
        .collect()
}

/// Count the number of parameters in a CALLABLE_DEF's PARAM_LIST.
/// Counts comma-separated items at paren-depth 1 (top-level of the list).
fn count_params_in_param_list(def_node: &syntax::SyntaxNode) -> usize {
    use parser::SyntaxKind as SK;
    // Find the first PARAM_LIST child node.
    let param_list = def_node.children().find(|n| ast::ParamList::can_cast(n.kind()));
    let Some(param_list) = param_list else {
        return 0;
    };

    // Count comma-separated items: if non-empty, count = commas + 1.
    let mut has_content = false;
    let mut comma_count: usize = 0;
    let mut depth: u32 = 0;
    for tok in param_list.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::L_PAREN => depth += 1,
            SK::R_PAREN => depth = depth.saturating_sub(1),
            SK::COMMA if depth == 1 => comma_count += 1,
            _ if depth == 1 && !tok.kind().is_trivia() => has_content = true,
            _ => {}
        }
    }
    if has_content {
        comma_count + 1
    } else {
        0
    }
}

/// Check if a TYPE_* node in a CALLABLE_DEF is preceded by a `->` token
/// (indicating it's a return type annotation, not a full inline signature).
fn has_preceding_arrow(parent: &syntax::SyntaxNode, type_node: &syntax::SyntaxNode) -> bool {
    use parser::SyntaxKind as SK;
    let type_offset = type_node.text_range().start();
    // Scan parent's direct children for R_ARROW before the type node.
    for el in parent.children_with_tokens() {
        if let Some(tok) = el.as_token() {
            if tok.text_range().start() >= type_offset {
                break;
            }
            if tok.kind() == SK::R_ARROW {
                return true;
            }
        }
    }
    false
}

/// Parse a type from source text by running the CST type parser.
/// Used for extracting types from Pat::Typed's ty_span.
pub(crate) fn type_from_type_text(text: &str) -> Ty {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ty::error();
    }
    // Use CST parser for full fidelity.
    // Wrap as `type _t = T` so the type gets parsed as a TYPE_ALIAS_DEF.
    let full = format!("type _t = {trimmed}\n");
    let (root, _) = syntax::parse_text(&full);
    if let Some(type_node) = find_type_child_deep(&root) {
        return type_from_cst_node(&type_node);
    }
    // Fallback
    Ty::named(trimmed.to_string())
}

/// Find first TYPE_* node anywhere in the tree (deep search).
fn find_type_child_deep(node: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    node.descendants().find(|c| {
        c != node
            && matches!(
                c.kind(),
                SK::TYPE_NAMED
                    | SK::TYPE_VAR
                    | SK::TYPE_APP
                    | SK::TYPE_TUPLE
                    | SK::TYPE_ARROW
                    | SK::TYPE_FORALL
                    | SK::TYPE_EXISTENTIAL
                    | SK::TYPE_EFFECT
            )
    })
}

/// Parse a constraint expression from source text (for assert propagation).

/// Find the first keyword token text in a CST node.
fn first_keyword_text(node: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    // Skip VISIBILITY nodes and trivia tokens to find the item keyword.
    for el in node.children_with_tokens() {
        match el {
            rowan::NodeOrToken::Node(n) if ast::Visibility::can_cast(n.kind()) => continue,
            rowan::NodeOrToken::Token(t) => {
                let k = t.kind();
                if k.is_trivia() || k == SK::IDENT || k == SK::TY_VAR {
                    continue;
                }
                return Some(t.text().to_string());
            }
            _ => continue,
        }
    }
    None
}

/// Find the first type expression node (searches descendants).
fn find_type_child(node: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    // Search direct children only (not descendants) to avoid picking up
    // TYPE_* nodes inside PARAM_LIST when we want the top-level type
    // (e.g. return type or inline signature).
    node.children().find(|c| c.kind().is_type_node())
}

/// Find the type expression node after `=` in a definition.
fn find_type_after_eq(node: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut found_eq = false;
    for child in node.children_with_tokens() {
        if let Some(tok) = child.as_token() {
            if tok.kind() == SK::EQ {
                found_eq = true;
                continue;
            }
        }
        if found_eq {
            if let Some(n) = child.into_node() {
                return Some(n);
            }
        }
    }
    None
}

/// Extract struct fields from CST: `{ field_name : type, ... }`
fn extract_struct_fields_from_cst(node: &syntax::SyntaxNode) -> HashMap<String, Ty> {
    use parser::SyntaxKind as SK;
    let mut fields = HashMap::new();
    let mut in_braces = false;
    let mut current_field: Option<String> = None;
    let mut after_colon = false;
    // When a type Node is consumed, skip all tokens inside it
    // to avoid treating inner IDENTs (e.g. "int") as field names.
    let mut skip_until: Option<usize> = None;

    for tok_or_node in node.descendants_with_tokens() {
        // Skip tokens inside already-consumed type nodes.
        if let Some(end) = skip_until {
            let offset = usize::from(tok_or_node.text_range().start());
            if offset < end {
                continue;
            }
            skip_until = None;
        }
        match tok_or_node {
            rowan::NodeOrToken::Token(tok) => match tok.kind() {
                SK::L_CURLY => {
                    in_braces = true;
                }
                SK::R_CURLY => {
                    if let Some(name) = current_field.take() {
                        fields.insert(name, Ty::error());
                    }
                    in_braces = false;
                }
                SK::IDENT if in_braces && !after_colon && current_field.is_none() => {
                    current_field = Some(tok.text().to_string());
                }
                SK::COLON if in_braces && current_field.is_some() => {
                    after_colon = true;
                }
                SK::IDENT if in_braces && after_colon && current_field.is_some() => {
                    // Type name token — use as type
                    if let Some(name) = current_field.take() {
                        fields.insert(name, Ty::named(tok.text().to_string()));
                    }
                    after_colon = false;
                }
                SK::TY_VAR if in_braces && after_colon && current_field.is_some() => {
                    // Type variable token ('a, 'b, etc.)
                    if let Some(name) = current_field.take() {
                        fields.insert(name, Ty::param(tok.text().to_string()));
                    }
                    after_colon = false;
                }
                SK::COMMA if in_braces => {
                    if let Some(name) = current_field.take() {
                        fields.insert(name, Ty::error());
                    }
                    after_colon = false;
                }
                _ => {}
            },
            rowan::NodeOrToken::Node(n) if in_braces && after_colon && current_field.is_some() => {
                let is_type = matches!(
                    n.kind(),
                    SK::TYPE_NAMED
                        | SK::TYPE_VAR
                        | SK::TYPE_APP
                        | SK::TYPE_TUPLE
                        | SK::TYPE_ARROW
                        | SK::TYPE_FORALL
                );
                if is_type {
                    if let Some(name) = current_field.take() {
                        fields.insert(name, type_from_cst_node(&n));
                    }
                    after_colon = false;
                    // Skip all descendants inside this type node.
                    skip_until = Some(usize::from(n.text_range().end()));
                }
            }
            _ => {}
        }
    }
    fields
}

/// Extract type parameter names from CST (from `(params)` after name).
fn extract_type_params_from_cst(node: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    // Look for PARAM_LIST child or TY_VAR tokens
    for child in node.children() {
        if ast::ParamList::can_cast(child.kind()) {
            return child
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| t.kind() == SK::TY_VAR || t.kind() == SK::IDENT)
                .map(|t| t.text().to_string())
                .collect();
        }
    }
    Vec::new()
}

/// Extract IDENT tokens from inside braces `{ ... }`.
fn extract_braced_idents(node: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    let mut result = Vec::new();
    let mut in_braces = false;
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::L_CURLY => in_braces = true,
            SK::R_CURLY => in_braces = false,
            SK::IDENT if in_braces => result.push(tok.text().to_string()),
            _ => {}
        }
    }
    result
}

/// Build a `BitfieldInfo` from a CST NAMED_DEF node for a bitfield.
///
/// Extracts the underlying type from the `: bits(N)` annotation, then
/// scans the braced body for field definitions like `HI : 7 .. 4` or
/// `LO : 3`, computing each field's bitvector width.
fn bitfield_info_from_cst(node: &syntax::SyntaxNode) -> Option<BitfieldInfo> {
    use parser::SyntaxKind as SK;

    // 1. Underlying type: the TYPE_* child from `: bits(N)`
    let underlying = find_type_child(node).map(|n| type_from_cst_node(&n))?;

    // 2. Fields: scan tokens inside { } for `IDENT : NUM .. NUM` or
    //    `IDENT : NUM .. NUM @ NUM .. NUM` (concat ranges) or `IDENT : NUM`.
    let mut fields = HashMap::new();
    let mut in_braces = false;
    let mut field_name: Option<String> = None;
    let mut after_colon = false;
    let mut range_nums: Vec<i64> = Vec::new();

    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::L_CURLY => {
                in_braces = true;
            }
            SK::R_CURLY => {
                // Flush last field
                if let Some(name) = field_name.take() {
                    fields.insert(name, bitfield_width_from_ranges(&range_nums));
                    range_nums.clear();
                }
                in_braces = false;
            }
            SK::IDENT if in_braces && !after_colon => {
                // Flush previous field if any
                if let Some(prev) = field_name.take() {
                    fields.insert(prev, bitfield_width_from_ranges(&range_nums));
                    range_nums.clear();
                }
                field_name = Some(tok.text().to_string());
            }
            SK::COLON if in_braces && field_name.is_some() => {
                after_colon = true;
                range_nums.clear();
            }
            SK::NUM_LIT if in_braces && after_colon => {
                if let Ok(n) = tok.text().parse::<i64>() {
                    range_nums.push(n);
                }
            }
            SK::COMMA if in_braces => {
                if let Some(name) = field_name.take() {
                    fields.insert(name, bitfield_width_from_ranges(&range_nums));
                    range_nums.clear();
                }
                after_colon = false;
            }
            _ => {}
        }
    }

    let typed_fields: HashMap<String, Ty> = fields
        .into_iter()
        .map(|(name, width)| {
            let ty = match width {
                Some(1) => Ty::named("bit".to_string()),
                Some(w) => bits_ty(w),
                None => Ty::error(),
            };
            (name, ty)
        })
        .collect();

    Some(BitfieldInfo { underlying, fields: typed_fields })
}

/// Compute field width from collected range numbers.
/// `[7, 4]` → Some(4), `[3]` → Some(1), `[31, 16, 7, 0]` → Some(24) (concat).
fn bitfield_width_from_ranges(nums: &[i64]) -> Option<i64> {
    if nums.is_empty() {
        return None;
    }
    if nums.len() == 1 {
        return Some(1); // single bit index
    }
    // Pairs of (high, low) ranges, possibly concatenated via @
    let mut total: i64 = 0;
    let mut i = 0;
    while i + 1 < nums.len() {
        let high = nums[i];
        let low = nums[i + 1];
        total += (high - low).abs() + 1;
        i += 2;
    }
    if i < nums.len() {
        // Odd number: last one is a single-bit field
        total += 1;
    }
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

/// Extract union variants from CST: `{ Name : Type, ... }`.
fn extract_union_variants_from_cst(node: &syntax::SyntaxNode) -> Vec<(String, Ty)> {
    use parser::SyntaxKind as SK;
    let mut variants = Vec::new();
    let mut in_braces = false;
    let mut current_name: Option<String> = None;
    // After we consume a type for a variant (via TYPE_* node or IDENT/TY_VAR
    // token), set this flag so that IDENT tokens nested inside that type
    // node's descendants are not mistaken for new variant names.
    // Cleared on COMMA or R_CURLY (start of next variant or end of list).
    let mut type_consumed = false;

    for tok_or_node in node.descendants_with_tokens() {
        match tok_or_node {
            rowan::NodeOrToken::Token(tok) => match tok.kind() {
                SK::L_CURLY => in_braces = true,
                SK::R_CURLY => {
                    if let Some(name) = current_name.take() {
                        variants.push((name, Ty::named("unit")));
                    }
                    in_braces = false;
                    type_consumed = false;
                }
                SK::COMMA => {
                    if let Some(name) = current_name.take() {
                        // No type annotation — default to unit.
                        variants.push((name, Ty::named("unit")));
                    }
                    type_consumed = false;
                }
                SK::IDENT if in_braces && current_name.is_none() && !type_consumed => {
                    current_name = Some(tok.text().to_string());
                }
                SK::TY_VAR if in_braces && current_name.is_some() => {
                    // Type variable as variant payload: `Some : 'a`
                    if let Some(name) = current_name.take() {
                        variants.push((name, Ty::param(tok.text().to_string())));
                        type_consumed = true;
                    }
                }
                SK::COLON if in_braces && current_name.is_some() => {
                    // After `:`, next comes the type. Handled below by
                    // TYPE_* node or IDENT token.
                }
                SK::IDENT if in_braces && current_name.is_some() => {
                    // Second IDENT after `:` — this is the type name.
                    if let Some(name) = current_name.take() {
                        variants.push((name, Ty::named(tok.text().to_string())));
                        type_consumed = true;
                    }
                }
                _ => {}
            },
            rowan::NodeOrToken::Node(n) if in_braces && current_name.is_some() => {
                let is_type = matches!(
                    n.kind(),
                    SK::TYPE_NAMED
                        | SK::TYPE_VAR
                        | SK::TYPE_APP
                        | SK::TYPE_TUPLE
                        | SK::TYPE_ARROW
                        | SK::TYPE_FORALL
                );
                if is_type {
                    if let Some(name) = current_name.take() {
                        variants.push((name, type_from_cst_node(&n)));
                        type_consumed = true;
                    }
                }
            }
            _ => {}
        }
    }
    variants
}

/// Try to extract a mapping scheme from a CST type node.
fn mapping_scheme_from_cst(type_node: &syntax::SyntaxNode) -> Option<MappingScheme> {
    use parser::SyntaxKind as SK;

    // Strip forall wrapper if present
    let mut quantifiers = Vec::new();
    let mut constraints = Vec::new();
    let body_node = collect_forall_from_cst(type_node, &mut quantifiers, &mut constraints);
    let inner = body_node.as_ref().unwrap_or(type_node);

    // Mapping type: `lhs <-> rhs` parsed as TYPE_ARROW.
    // Check for DOUBLE_ARROW token to confirm it's a mapping (not -> function).
    if !ast::TypeArrow::can_cast(inner.kind()) {
        return None;
    }
    let has_bidir = inner
        .children_with_tokens()
        .any(|el| el.as_token().map(|t| t.kind() == SK::DOUBLE_ARROW).unwrap_or(false));
    if !has_bidir {
        return None;
    }

    let children: Vec<_> = inner.children().collect();
    if children.len() < 2 {
        return None;
    }
    let lhs = type_from_cst_node(&children[0]);
    let rhs = type_from_cst_node(&children[children.len() - 1]);

    Some(MappingScheme { quantifiers, constraints, lhs, rhs })
}

/// Extract a constraint expression from a CST type node.
fn constraint_from_cst_node(node: &syntax::SyntaxNode) -> ConstraintExpr {
    let tokens: Vec<(parser::SyntaxKind, String)> = node
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .map(|tok| (tok.kind(), tok.text().to_string()))
        .collect();
    if tokens.is_empty() {
        return ConstraintExpr::Unsupported;
    }
    parse_constraint_from_tokens(&tokens)
}

/// Returns true only if `ty` is something we KNOW cannot be a record/bitfield
/// (e.g. int, bool, bits(N), etc.). Returns false for cross-file type
/// aliases or otherwise-unknown type names — those should NOT be flagged as
/// "not a record" because we can't tell.
fn is_known_non_record(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)) | TyKind::Param(_) => false,
        TyKind::Scalar(_) => true, // All scalars are known non-records
        TyKind::FnPtr { .. } => true,
        TyKind::Tuple(_) => true,
        TyKind::Adt(_name, _) => false, // ADTs might be records — we don't know without lookup
        TyKind::App { name, .. } => {
            matches!(name.as_str(), "bits" | "vector" | "list" | "range" | "atom")
        }
        TyKind::Exist { .. } => false, // existential might be a record
        TyKind::Bidir { .. } => true,  // bidir is never a record
        TyKind::Abstract { .. } => true, // abstract types are opaque, not records
    }
}

/// Decides whether `name` in a pattern position should be treated as a
/// variable binding (`true`) or as a nullary constructor match (`false`).
///
/// The uppercase heuristic is always applied: names starting with an
/// ASCII uppercase letter are treated as constructors regardless of
/// workspace context. The workspace context enhances detection of
/// *lowercase* constructors (e.g. `float_class_negative_inf`) via the
/// `pattern_constants` set, but uppercase names are never bindings in
/// idiomatic Sail.
fn is_pattern_binding(
    name: &str,
    pattern_constants: &HashSet<String>,
    _workspace_known_constructors: bool,
) -> bool {
    if pattern_constants.contains(name) {
        return false;
    }
    if PRELUDE_CONSTRUCTORS.contains(&name) {
        return false;
    }
    if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cst_env(source: &str) -> TopLevelEnv {
        let (root, _) = syntax::parse_text(source);
        TopLevelEnv::from_cst(&root).0
    }

    #[test]
    fn cst_val_spec_produces_function_scheme() {
        let env = cst_env("val foo : int -> int\n");
        assert!(env.functions.contains_key("foo"), "expected function 'foo'");
        let schemes = &env.functions["foo"];
        assert_eq!(schemes.len(), 1);
    }

    #[test]
    fn cst_enum_produces_members() {
        let env = cst_env("enum color = { Red, Green, Blue }\n");
        assert!(env.enums.contains_key("color"));
        let members = &env.enums["color"];
        assert_eq!(members.len(), 3);
        assert!(members.contains(&"Red".to_string()));
        assert!(members.contains(&"Green".to_string()));
        assert!(members.contains(&"Blue".to_string()));
        // Members should also be in values
        assert!(env.values.contains_key("Red"));
    }

    #[test]
    fn cst_register_produces_value_and_register() {
        let env = cst_env("register PC : bits(64)\n");
        assert!(env.registers.contains_key("PC"));
        assert!(env.values.contains_key("PC"));
    }

    #[test]
    fn cst_type_alias() {
        let env = cst_env("type myint = int\n");
        assert!(env.type_aliases.contains_key("myint"));
    }

    #[test]
    fn cst_struct_produces_record() {
        let env = cst_env("struct Point = { x : int, y : int }\n");
        assert!(env.records.contains_key("Point"));
        assert!(env.known_field_names.contains("x"));
        assert!(env.known_field_names.contains("y"));
    }

    #[test]
    fn cst_let_produces_value() {
        let env = cst_env("let x : int = 42\n");
        assert!(env.values.contains_key("x"));
    }

    #[test]
    fn cst_val_spec_function_count() {
        let source = "val foo : int -> int\nval bar : bool -> bool\n";
        let cst = cst_env(source);
        assert_eq!(cst.functions.len(), 2);
        assert!(cst.functions.contains_key("foo"));
        assert!(cst.functions.contains_key("bar"));
    }

    #[test]
    fn cst_multiple_definitions() {
        let source = "\
val add : (int, int) -> int
enum color = { Red, Green, Blue }
type myint = int
register PC : bits(64)
let x : int = 42
";
        let cst = cst_env(source);
        // 2 functions: `add` from val spec + auto-generated `num_of_color`
        assert_eq!(cst.functions.len(), 2, "functions");
        assert_eq!(cst.enums.len(), 1, "enums");
        assert_eq!(cst.type_aliases.len(), 1, "aliases");
        assert_eq!(cst.registers.len(), 1, "registers");
    }

    #[test]
    fn inference_result_write_and_read() {
        let mut result = InferenceResult::default();
        let id = hir_def::expr_id_from_raw(1);
        let ty = Ty::named("int");
        result.write_expr_ty(id, ty.clone());
        assert_eq!(result.expr_ty(id).unwrap().display_text(), "int");
        assert_eq!(result.expr_count(), 1);
    }

    #[test]
    fn inference_result_merge() {
        let mut a = InferenceResult::default();
        a.write_expr_ty(hir_def::expr_id_from_raw(1), Ty::named("int"));
        let mut b = InferenceResult::default();
        b.write_expr_ty(hir_def::expr_id_from_raw(2), Ty::named("bool"));
        a.merge(b);
        assert_eq!(a.expr_count(), 2);
    }

    #[test]
    fn inference_result_to_type_check_result() {
        let source = "function f() = 42\n";
        let (cst_root, _) = syntax::parse_text(source);
        let bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);
        let entry = bodies.entries().first().expect("no callable found");
        let body = &entry.body;
        let source_map = &entry.source_map;
        let mut result = InferenceResult::default();
        result.write_expr_ty(body.root(), Ty::named("int"));
        let tcr = result.to_type_check_result(body, source_map);
        assert!(
            tcr.expr_ty(body.root()).is_some(),
            "expected InferenceResult to have entry for root expr"
        );
    }

    #[test]
    fn cst_forall_scheme_has_params() {
        let env = cst_env("val zeroes : forall 'n. unit -> bits('n)\n");
        let schemes = &env.functions["zeroes"];
        assert_eq!(schemes.len(), 1);
        assert_eq!(schemes[0].params.len(), 1, "expected 1 param (unit)");
        assert_eq!(schemes[0].quantifiers, vec!["'n".to_string()]);
    }

    /// Helper: run type inference on source and return a display of
    /// all inferred expression types, sorted by ExprId.
    fn infer_display(source: &str) -> String {
        let (cst_root, _) = syntax::parse_text(source);
        let (env, pattern_constants) = TopLevelEnv::from_cst(&cst_root);
        let callable_bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);
        // Per-callable inference (RA infer per DefWithBodyId)
        let mut result = TypeCheckResult::default();
        for entry in callable_bodies.entries() {
            // Fresh context per callable
            let mut ctx = InferenceContext::new_for_body(
                source,
                entry.body.clone(),
                entry.source_map.clone(),
                env.clone(),
                pattern_constants.clone(),
            );
            if entry.body.mapping_arms.is_empty() {
                ctx.infer_callable_body_hir(&entry.name, entry);
            } else {
                ctx.infer_mapping_body_hir(&entry.name, entry);
            }
            let per_callable = ctx.finish_query();
            result.diagnostics.extend(per_callable.diagnostics);
            // Merge type_of_expr for display
            for (id, ty) in per_callable.type_of_expr.iter() {
                result.type_of_expr.insert(id, ty.clone());
            }
        }

        let mut lines: Vec<String> = Vec::new();
        let mut entries: Vec<_> = result.type_of_expr.iter().collect();
        entries.sort_by_key(|(id, _)| format!("{:?}", id));
        for (id, ty) in entries {
            if !ty.is_unknown() {
                lines.push(format!("{:?}: {}", id, ty.display_text()));
            }
        }
        lines.join("\n")
    }

    #[test]
    fn expect_infer_arithmetic() {
        let display = infer_display("function f(x : int) -> int = x + 1\n");
        // Just verify some types are inferred (exact ExprIds are unstable)
        assert!(!display.is_empty(), "should infer at least one expression type");
        assert!(display.contains("int"), "should infer int types, got: {display}");
    }

    #[test]
    fn expect_infer_boolean() {
        let display = infer_display("function g(b : bool) -> bool = b\n");
        assert!(display.contains("bool"), "should infer bool type, got: {display}");
    }

    #[test]
    fn expect_infer_with_val_spec() {
        let display = infer_display("val add : (int, int) -> int\nfunction add(x, y) = x + y\n");
        assert!(display.contains("int"), "should infer int from val spec, got: {display}");
    }
}
