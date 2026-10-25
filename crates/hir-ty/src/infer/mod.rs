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

mod config;
mod constraint;
pub mod declaration_constraints;
pub(crate) mod dependencies;
pub use config::configured_constraint_status;
mod fallback;
use constraint::*;
/// Top-level type environment (TopLevelEnv, RecordInfo, BitfieldInfo).
pub mod env;
pub(crate) use env::{BitfieldInfo, RecordInfo, TopLevelEnv};
// Re-export env helper functions used by sibling modules (expr.rs, numeric.rs, z3_solver.rs)
pub(super) use env::{
    bits_ty, infer_literal_type, parse_int_literal, promote_record_ty, register_ty, ty_to_match_ty,
    ty_to_match_ty_with_subst, vector_ty,
};
mod boolean;
/// Type coercion — implicit type conversions (subtype coercions).
pub(super) mod coerce;
/// Existential type inference — skolemization and witness extraction.
pub mod existential;
mod integer;
mod kinds;
/// Bidirectional mapping type checking (Sail-specific).
pub mod mapping;
pub(super) mod nexp_simplify;
pub(crate) mod numeric;
mod obligation_stats;
mod refinement;
mod runtime_witness;
mod solver;
pub use obligation_stats::{ObligationStats, UnknownConstraintReason};
mod substitution;
pub(super) mod subtype;
/// Type well-formedness checking.
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
    type_variables: HashSet<String>,

    constraints: Vec<ConstraintExpr>,
    undo_log: Vec<LocalEnvUndo>,
    contradiction: std::cell::Cell<Option<bool>>,
}

#[derive(Clone, Debug)]
enum LocalEnvUndo {
    PushScope,
    Define { name: String, previous: Option<Ty> },
    AddConstraint,
    DefineTypeVariable(String),
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
                    TyArg::Bool(c) => GenericArg::Value(c.to_text()),
                    TyArg::Order(v) | TyArg::Value(v) => GenericArg::Value(v.clone()),
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
pub enum Obligation {
    All(Vec<Obligation>),
    Assuming {
        assumptions: Vec<ConstraintExpr>,
        goal: Box<Obligation>,
    },
    /// A numeric constraint that must hold (e.g., `'n + 'm < 64`).
    NumericConstraint(ConstraintExpr),
    /// Two numeric expressions that must be equal (e.g., bitvector
    /// widths in `bits('n)` operations).
    WidthEquality(NumericExpr, NumericExpr),
}

impl Obligation {
    pub fn constraint(&self) -> ConstraintExpr {
        match self {
            Self::All(goals) => ConstraintExpr::And(goals.iter().map(Self::constraint).collect()),
            Self::Assuming { assumptions, goal } => ConstraintExpr::Or(vec![
                ConstraintExpr::Not(Box::new(ConstraintExpr::And(assumptions.clone()))),
                goal.constraint(),
            ]),
            Self::NumericConstraint(c) => c.clone(),
            Self::WidthEquality(lhs, rhs) => {
                ConstraintExpr::Compare { lhs: lhs.clone(), op: CompareOp::Eq, rhs: rhs.clone() }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObligationCause {
    Explicit,
    Compatibility,
    /// Structural recovery accepted a relation without proving it.
    Recovery,
}

/// A deferred goal together with the lexical evidence available at its origin.
#[derive(Clone, Debug)]
pub struct ScopedObligation {
    pub goal: Obligation,
    pub universe: u32,
    pub rigid_variables: Vec<String>,
    pub cause: ObligationCause,
    pub origin: Option<hir_def::ExprId>,
    pub assumptions: std::sync::Arc<[ConstraintExpr]>,
    pub substitution: Subst,
    /// Present only after final solving returned Unknown.
    pub unknown_reason: Option<UnknownConstraintReason>,
    /// (actual, expected) retained when shape recovery erased the numeric goal.
    pub recovery_types: Option<(Ty, Ty)>,
    /// Normalized (actual, expected) at an unsupported compatibility boundary.
    /// Diagnostic evidence only; never assumptions or a subtype proof.
    pub compatibility_types: Option<(Ty, Ty)>,
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
    /// A source annotation on a local binding. Its names are interpreted
    /// in each continuation's lexical scope, unlike an instantiated signature.
    LexicalType(Ty),
}

impl Expectation {
    /// The expected type, if there is one and it is usable.
    pub(super) fn to_option(&self) -> Option<Ty> {
        match self {
            Expectation::None => None,
            Expectation::HasType(ty) | Expectation::LexicalType(ty) if ty.is_error() => None,
            Expectation::HasType(ty) | Expectation::LexicalType(ty) => Some(ty.clone()),
        }
    }
}

//
// Extracted to `infer/unify.rs` ().
// Re-exported here for backward compatibility.
pub mod unify;
pub use unify::{InferenceTable, InferenceTableSnapshot};

/// Normalize a receiver's aliases in its declaration context. Keep this
/// projection name-specific just like the environment used by body inference.
pub fn normalize_type_at(
    db: &dyn salsa::Database,
    at: hir_def::source_order::Declaration,
    ty: &crate::ty::Ty,
) -> crate::ty::Ty {
    let env = crate::infer::dependencies::ordered::environment(
        db,
        at,
        crate::infer::dependencies::names_in(&ty.display_text()),
    );
    let mut table = crate::infer::InferenceTable::with_aliases(env.0.env.type_aliases.clone());
    table.set_alias_schemes(env.0.env.alias_schemes.clone());
    table.normalize_alias_deep(ty)
}

/// Per-callable inference result, keyed by `ExprId`/`PatId`.
#[derive(Clone, Debug, Default)]
pub struct InferenceResult {
    /// The actual leaf chosen for an overloaded callee expression. Keep its
    /// name rather than reconstructing identity from the instantiated type.
    pub selected_overloads: FxHashMap<hir_def::ExprId, String>,
    pub type_of_expr: ArenaMap<hir_def::ExprId, Ty>,
    pub type_of_pat: ArenaMap<hir_def::PatId, Ty>,
    pub type_of_binding: ArenaMap<hir_def::BindingId, Ty>,
    pub method_resolutions:
        FxHashMap<hir_def::ExprId, (hir_def::item_id::FunctionId, base_db::FileId)>,
    pub field_resolutions: FxHashMap<hir_def::ExprId, hir_def::DefId>,
    /// Instantiated missing fields for record-literal editing. Anonymous
    /// literals can offer an assist without emitting a missing-fields error.
    pub missing_record_fields: FxHashMap<hir_def::ExprId, Vec<(String, String)>>,
    pub variant_resolutions: FxHashMap<hir_def::ExprOrPatId, hir_def::DefId>,
    pub expr_adjustments: FxHashMap<hir_def::ExprId, Vec<Adjustment>>,
    pub type_mismatches: FxHashMap<hir_def::ExprOrPatId, TypeMismatch>,
    /// Expressions that diverge: `return`, `throw`, `exit`, and any block
    /// that ends in one.
    ///
    /// Their type is `Ty::error()`, because this checker spells *never* and
    /// *unknown* the same way — and a reader of the result cannot tell a
    /// block that cannot fall through from one whose type nobody worked
    /// out. `analysis_stats` counted 26 untyped blocks on sail-riscv, most
    /// of them `never` and not a gap in anything; the count that drives the
    /// inference-coverage loop has to leave them out or the loop chases
    /// them.
    pub diverging_exprs: rustc_hash::FxHashSet<hir_def::ExprId>,
    pub has_errors: bool,
    pub diagnostics: Vec<InferenceDiagnostic>,
    pub unsolved_constraints: Vec<ScopedObligation>,
    pub refuted_constraints: Vec<ScopedObligation>,
    /// Final outcomes of committed deferred obligations, not all subtype checks.
    pub obligation_stats: ObligationStats,
    /// Checks that run but are deliberately not shown.
    ///
    /// `semantic_diagnostics` renders `diagnostics` (the structured channel)
    /// and `type_mismatches`; anything here reaches no user. One check is
    /// left: the `unverified-constraint` hint. Everything else that used to
    /// be here is a structured `InferenceDiagnostic` now.
    pub legacy_diagnostics: Vec<Diagnostic>,
    pub legacy_diagnostic_byte_spans: Vec<(usize, usize)>,
    pub bodies: Option<Arc<hir_def::bodies::CallableBodies>>,
    /// Where the callable started when this result was computed.
    ///
    /// A result survives an edit above its callable — that is the point of
    /// `callable_body`'s fingerprint — so the few raw spans it carries
    /// (`derived_from`, which upstream renders as "constraint from ...")
    /// belong to that revision. The source map knows where the callable is
    /// now; the difference is how far they have to move.
    pub def_base: usize,
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

/// Why a name's subrange pieces cannot bind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubrangeProblem {
    /// One range, but it does not reach bit 0.
    NotFromZero { lowest: usize },
    /// More than one range after merging: a bit in between is unbound.
    Gap { missing: usize },
}

/// Typed inference diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceDiagnostic {
    /// A known type cannot be used by this pattern or expression form.
    InvalidTypeUse {
        node: hir_def::ExprOrPatId,
        message: String,
    },
    UnresolvedIdent {
        expr: hir_def::ExprId,
        name: String,
    },
    UnresolvedField {
        expr: hir_def::ExprId,
        receiver: Ty,
        name: String,
        method_with_same_name_exists: bool,
        /// The receiver is positively *not* a record — a union, an enum,
        /// a scalar. Upstream says something else then: the finding is
        /// about the type, not about a missing field
        /// (`type_check.ml:3678`).
        receiver_is_not_a_record: bool,
        /// On the left of an `=`, where upstream's `LE_field` has its own
        /// sentence (`:3611`).
        in_assign_target: bool,
        /// Whether this handle could see what fields the receiver has.
        ///
        /// A record declared in another file resolves by name and not by
        /// body, so the field set is unknown and the miss says nothing.
        /// When the declaration *is* in reach, a field that is not in it
        /// does not exist — which is the only case worth reporting.
        receiver_fields_known: bool,
    },
    MismatchedArgCount {
        call_expr: hir_def::ExprId,
        expected: usize,
        found: usize,
    },
    /// A function's parameter list disagrees with the `val` that declares it.
    ///
    /// `MismatchedArgCount` is about a *call*; this is about the definition.
    /// Nothing checked it: the signature is in the item tree and inference
    /// never read it, so dropping a parameter from a definition whose `val`
    /// still declares it went unreported — 25 of the 67 mutants
    /// `mutation_check` missed.
    MismatchedParamCount {
        body_expr: hir_def::ExprId,
        expected: usize,
        found: usize,
    },
    ExpectedFunction {
        call_expr: hir_def::ExprId,
        found: Ty,
    },
    EffectViolation {
        expr: hir_def::ExprId,
        effect: hir_def::EffectTag,
        context: &'static str,
    },
    UnsolvedConstraint {
        expr: hir_def::ExprId,
        constraint: String,
    },
    IncompleteMatch {
        expr: hir_def::ExprId,
        missing_arms: Vec<String>,
    },
    /// A match arm whose pattern an earlier arm already covers.
    RedundantMatchArm {
        pat: hir_def::PatId,
    },
    MissingFields {
        expr: hir_def::ExprId,
        record_name: String,
        missing: Vec<String>,
        field_types: Vec<String>,
    },
    UnusedVariable {
        pat: hir_def::PatId,
        name: String,
    },
    RemoveTrailingReturn {
        return_expr: hir_def::ExprId,
    },
    /// A statement after one that always diverges. Reported where the
    /// divergence is known — `Diverges::Always` is inference's own state,
    /// and block inference stops at it either way.
    UnreachableStatement {
        expr: hir_def::ExprId,
    },
    RemoveUnnecessaryElse {
        if_expr: hir_def::ExprId,
    },
    ConcatTypeMismatch {
        expr: hir_def::ExprId,
        message: String,
    },
    ConstraintViolation {
        expr: hir_def::ExprId,
        constraint: String,
        derived_from: Vec<parser::Span>,
    },
    UnrecoverableImplicit {
        expr: hir_def::ExprId,
        message: String,
    },
    CallConstraintViolation {
        call_expr: hir_def::ExprId,
        constraint: String,
        derived_from: Vec<parser::Span>,
        /// The constraint was shown to have no solution, not merely left
        /// undecided. `'m == 8 * 'n` with `'m` fixed at 65 is the first;
        /// a constraint the solver did not reach is the second.
        proved_false: bool,
    },
    UnresolvedCallQuantifiers {
        call_expr: hir_def::ExprId,
        id: String,
        quants: Vec<String>,
        signature: Option<String>,
    },
    NoOverloading {
        call_expr: hir_def::ExprId,
        name: String,
        /// Zero candidates rather than "candidates exist, none matched".
        /// Upstream keeps these apart — `Err_no_function_type` against
        /// `Err_no_overloading` (`type_error.mli`) — and says "Function
        /// {name} not found" for the first.
        no_function: bool,
        /// The nearest name the environment does know, by upstream's own
        /// rule (`find_closest`, `type_error.ml:244`).
        closest: Option<String>,
    },
    /// The name is bound, and to something that is not callable.
    NotAFunction {
        call_expr: hir_def::ExprId,
        name: String,
    },
    MappingBindingMismatch {
        expr: hir_def::ExprId,
        name: String,
        side: &'static str,
    },
    DuplicateBinding {
        pat: hir_def::PatId,
        name: String,
        /// A duplicate *field* in a struct pattern, which upstream words
        /// differently: `Duplicate field 'f1' in struct pattern`
        /// (`initial_check.ml:1360`) against `Duplicate binding for x in
        /// pattern` (`type_check.ml:1650`). Two checks, two sentences.
        in_struct_pattern: bool,
    },
    MissingPatternFields {
        pat: hir_def::PatId,
        record_name: String,
        missing: Vec<String>,
        field_types: Vec<String>,
    },
    /// A struct pattern naming a field the record does not declare.
    UnknownPatternField {
        pat: hir_def::PatId,
        record_name: String,
        name: String,
    },
    /// A name whose subrange pieces do not tile `bits(n .. 0)`.
    ///
    /// Upstream groups a concatenation's subranges *by the identifier they
    /// bind* and checks each group on its own
    /// (`each_pattern_vector_subranges`, `type_check.ml:1786-1806`), with two
    /// distinct messages — hence the two shapes here.
    NonContiguousSubrange {
        pat: hir_def::PatId,
        name: String,
        problem: SubrangeProblem,
    },
    /// `x[n .. m]` with the indices the wrong way round for the declared
    /// order. Carries an `ExprOrPatId` because upstream raises it from both
    /// positions: the expression `x[0 .. 2]` and the *pattern*
    /// `P_vector_subrange` (`type_check.ml:1763-1777`, raised at `:3095`).
    VectorSubrangeOrder {
        node: hir_def::ExprOrPatId,
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
        None
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
        None
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
    pub fn record_field_resolution(&mut self, expr_id: hir_def::ExprId, field_def: hir_def::DefId) {
        self.field_resolutions.insert(expr_id, field_def);
    }

    /// Record which variant/constructor an expression or pattern resolves to.
    pub fn record_variant_resolution(
        &mut self,
        id: impl Into<hir_def::ExprOrPatId>,
        variant_def: hir_def::DefId,
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
        self.selected_overloads.extend(other.selected_overloads);
        self.unsolved_constraints.extend(other.unsolved_constraints);
        self.refuted_constraints.extend(other.refuted_constraints);
        self.obligation_stats.merge(other.obligation_stats);
        self.legacy_diagnostics.extend(other.legacy_diagnostics);
        self.legacy_diagnostic_byte_spans.extend(other.legacy_diagnostic_byte_spans);
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
    ///
    /// A second renderer beside `HirDisplay` (`hir-ty/src/display.rs`),
    /// and they drifted: this one printed an unresolved inference variable
    /// as `?34` — this checker's own counter — after the other had stopped.
    /// Both say `_` now; merging them is a separate job and this comment is
    /// the marker for it.
    pub fn display_text(&self) -> String {
        let mut out = String::new();
        self.display_text_into(&mut out);
        out
    }

    fn display_text_into(&self, out: &mut String) {
        match self.kind() {
            TyKind::Error => out.push('_'),
            TyKind::Infer(_) => out.push_str(crate::ty::UNRESOLVED_VAR),
            TyKind::Scalar(s) => out.push_str(s.name()),
            TyKind::Named(text, _) => out.push_str(text),
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
    fn is_contradictory(&self) -> bool {
        if let Some(value) = self.contradiction.get() {
            return value;
        }
        let value = integer::contradictory(&self.constraints, &self.type_substitution());
        self.contradiction.set(Some(value));
        value
    }
    pub(super) fn define_type_binding(&mut self, name: &str, ty: Ty) {
        self.define(&format!("$type:{name}"), ty);
    }
    pub(super) fn type_substitution(&self) -> Subst {
        let mut subst = Subst::default();
        for name in &self.type_variables {
            if let Some(ty) = self.lookup(&format!("$type:{name}")) {
                subst.types.insert(name.clone(), ty.clone());
                if let TyKind::Param(value) = ty.kind() {
                    subst.values.insert(name.clone(), value.clone());
                }
            }
        }
        subst
    }
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn restore(&mut self, mark: usize) {
        self.contradiction.set(None);
        while self.undo_log.len() > mark {
            match self.undo_log.pop().expect("undo log length checked") {
                LocalEnvUndo::PushScope => {}
                LocalEnvUndo::DefineTypeVariable(name) => {
                    self.type_variables.remove(&name);
                }
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

    /// Copy another scope's bindings in, without disturbing this one's.
    /// Only a mapping clause's guard needs it: the guard may mention names
    /// bound by either side of the `<->`, which are checked separately.
    fn merge_bindings(&mut self, other: &LocalEnv) {
        for (name, ty) in &other.bindings {
            self.bindings.entry(name.clone()).or_insert_with(|| ty.clone());
        }
    }

    /// Upstream's `other_env`: an identifier a mapping pattern cannot type on
    /// its own takes the type the other side of the clause gave it
    /// (`infer_mpat`, type_check.ml:4494, and the two `check_mpexp` calls at
    /// 4682 that pass each side's env as the other's fallback).
    ///
    /// A fallback only. Where this side worked a type out, it keeps it even
    /// if the two disagree — that disagreement is the reason the two
    /// directions get separate scopes at all.
    fn fill_from_other_side(&mut self, other: &LocalEnv) {
        let names: Vec<(String, Ty)> = other
            .bindings
            .iter()
            .filter(|(_, ty)| !ty.is_error())
            .filter(|(name, _)| !self.bindings.get(*name).is_some_and(|own| !own.is_error()))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();
        for (name, ty) in names {
            self.define(&name, ty);
        }
    }

    fn define_type_variable(&mut self, name: &str) {
        if self.type_variables.insert(name.to_owned()) {
            self.undo_log.push(LocalEnvUndo::DefineTypeVariable(name.to_owned()));
        }
    }

    fn define(&mut self, name: &str, ty: Ty) {
        self.contradiction.set(None);
        let name = name.to_string();
        let previous = self.bindings.insert(name.clone(), ty);
        self.undo_log.push(LocalEnvUndo::Define { name, previous });
    }

    fn add_constraint(&mut self, constraint: ConstraintExpr) {
        self.contradiction.set(None);
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
            // Conditional numeric aliases are TYPE_NAMED nodes too. Their
            // first identifier is the condition, not the alias's value.
            if node.first_token().is_some_and(|t| t.kind() == SK::KW_IF) {
                return Ty::named(node.text().to_string().trim());
            }
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
            // `(T)` is just `T` — except when `T` is itself a tuple. Sail
            // distinguishes `(int, int) -> int` (two parameters) from
            // `((int, int)) -> int` (one tuple parameter), and collapsing the
            // extra layer would make the second look like the first.
            let collapse =
                items.len() == 1 && !matches!(items[0].kind(), crate::ty::TyKind::Tuple(_));
            if collapse {
                items.into_iter().next().unwrap()
            } else {
                Ty::tuple(items)
            }
        }
        SK::TYPE_ARROW => {
            // Distinguish `->` (function) from `<->` (bidir mapping)
            let is_bidir = node
                .children_with_tokens()
                .any(|el| el.as_token().is_some_and(|t| t.kind() == SK::DOUBLE_ARROW));
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
                        // `(A, B) -> C` is two parameters; `((A, B)) -> C` is
                        // one, and the extra parentheses are the only thing
                        // that says so. Flattening both alike made
                        // `val f : ((string, string)) -> string` look like a
                        // two-argument function at its call sites.
                        SK::TYPE_TUPLE => {
                            let kids: Vec<_> = lhs_node.children().collect();
                            if kids.len() == 1 && kids[0].kind() == SK::TYPE_TUPLE {
                                vec![type_from_cst_node(&kids[0])]
                            } else {
                                kids.iter().map(type_from_cst_node).collect()
                            }
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
            // A keyword-headed type has no name child, so its head has to be
            // read from the token *first*: `register(S)` otherwise takes the
            // argument `S` for the constructor and lowers to `S` with no
            // arguments, which then fails to unify with a `register(S)` built
            // anywhere else — while printing identically.
            let keyword_head = node
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| matches!(t.kind(), SK::KW_REGISTER))
                .map(|t| t.text().to_string());
            let (name, args_start) = match keyword_head {
                Some(kw) => (kw, 0), // all children are args
                None => match children.first().and_then(cst_ident_text) {
                    Some(n) => (n, 1), // skip the name child
                    None => (
                        children
                            .first()
                            .map(|c| c.text().to_string().trim().to_string())
                            .unwrap_or_default(),
                        0,
                    ),
                },
            };
            let mut args: Vec<TyArg> =
                children.iter().skip(args_start).map(type_arg_from_cst_node).collect();
            // Drop the order argument. Sail 0.20 parses `vector('n, ord, 'a)`
            // and `bitvector('n, ord)` (initial_check.ml:2388-2389) but the
            // typed AST carries `vector('n, 'a)` / `bitvector('n)`
            // (type_env.ml `builtin_typs`). Keeping it would make
            // `vector(3, dec, int)` and `vector(3, int)` distinct types.
            let order_position = match name.as_str() {
                "vector" if args.len() == 3 => Some(1),
                "bitvector" if args.len() == 2 => Some(1),
                _ => None,
            };
            // Dropped by arity, not by inspecting the argument: the order
            // may be a type variable (`vector('n, 'ord, bit)`), and removing
            // it in only some occurrences would make the two spellings
            // disagree.
            let text = if let Some(pos) = order_position {
                args.remove(pos);
                // Rebuild rather than keep the source text: leaving the order
                // in the display while dropping it from the arguments makes a
                // mismatch report read `vector(4, dec, T)` vs `vector(4, T)`
                // for two types that differ in something else entirely.
                let inner: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        TyArg::Type(t) => t.display_text(),
                        other => other.as_value_str().unwrap_or_default(),
                    })
                    .collect();
                format!("{name}({})", inner.join(", "))
            } else {
                node.text().to_string().trim().to_string()
            };
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
            // `{4, 8}` is a numeric set, not an existential: upstream has a
            // separate production for it (`Lcurly num_list Rcurly` ->
            // `ATyp_nset`, parser.mly:396). The grammar folds both spellings
            // into one node, so tell them apart by the `.` that separates an
            // existential's quantifiers from its body. Without this the type
            // lowers to an error, and an error-typed parameter list is the
            // signal `apply_callable_signature_metadata` takes as "no type
            // information, use the clause's" -- which silently replaced
            // `val f : {4,8} -> unit` with whatever arity the clause looked
            // like it had.
            let has_body = node
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::DOT);
            if !has_body {
                // Keep the exact set: its convex hull admits values that
                // cannot occur and creates false counterexamples at calls.
                let text = node.text().to_string();
                let items: Option<Vec<_>> = text
                    .trim()
                    .trim_start_matches("{|")
                    .trim_end_matches("|}")
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .split(',')
                    .map(|n| NumericExpr::integer_literal(n.trim()))
                    .collect();
                let Some(items) = items.filter(|items| !items.is_empty()) else {
                    return Ty::named("int");
                };
                let var = "'_set".to_string();
                let predicate =
                    ConstraintExpr::InSet { value: NumericExpr::Var(var.clone()), items };
                let inner = Ty::app("atom", vec![TyArg::Nexp(NumericExpr::Var(var.clone()))], "");
                let vars = vec![var];
                return existential_as_range(&vars, &predicate, &inner)
                    .unwrap_or_else(|| Ty::exist(vars, predicate, inner));
            }
            // The bound variables are a `TYPE_PARAM_LIST` and the
            // constraint is the type node between it and the body — the
            // grammar the parser now builds, the same one a `forall` gets.
            //
            // This was the second copy of the token heuristics: collect the
            // direct tokens, find the first comparison operator, walk back
            // to the comma before it to decide where the names stopped.
            let mut vars: Vec<String> = Vec::new();
            let mut kind_constraints = Vec::new();
            let mut constraint = ConstraintExpr::Bool(true);
            let mut children = node.children();
            let param_list = children.next().filter(|n| n.kind() == SK::TYPE_PARAM_LIST);
            let rest: Vec<syntax::SyntaxNode> = children.collect();

            if let Some(param_list) = &param_list {
                for param in param_list.children().filter(|n| n.kind() == SK::TYPE_PARAM) {
                    kind_constraints
                        .extend(natural_kind_constraints(&param).into_iter().map(|c| c.expr));
                    for name in type_param_names(&param) {
                        if !vars.contains(&name) {
                            vars.push(name);
                        }
                    }
                }
            }
            if rest.len() >= 2 {
                let tokens: Vec<(SK, String)> = rest[0]
                    .descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|t| !t.kind().is_trivia())
                    .map(|t| (t.kind(), t.text().to_string()))
                    .collect();
                if !tokens.is_empty() {
                    constraint = parse_constraint_from_tokens(&tokens);
                }
            }

            if !kind_constraints.is_empty() {
                kind_constraints.push(constraint);
                constraint = ConstraintExpr::And(kind_constraints);
            }
            // Parse body type
            let inner = node
                .children()
                .last()
                .map(|body| type_from_cst_node(&body))
                .unwrap_or_else(Ty::error);

            // `{'n, 0 <= 'n & 'n <= 4. atom('n)}` *is* `range(0, 4)`. Left
            // as an existential it was opaque to everything numeric, so
            // `let B : range(0, 3) = A` compared a range against a witness
            // nobody could see. The reduction only fires when the body is
            // exactly the bound variable and the constraint pins both ends
            // to literals — anything else keeps its existential.
            if let Some(range) = existential_as_range(&vars, &constraint, &inner) {
                return range;
            }
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
    let text = node.text().to_string();
    if matches!(text.trim(), "true" | "false" | "inc" | "dec") {
        return TyArg::numeric(text.trim());
    }
    // Parenthesized numeric expressions can be TYPE_APP in the lossless
    // CST. Their arithmetic structure, rather than that node tag, decides.
    if text.trim_start().starts_with('(') {
        if let Some(n) = NumericExpr::parse(text.trim()) {
            if !matches!(n, NumericExpr::Var(_) | NumericExpr::Symbol(_) | NumericExpr::App { .. })
            {
                return TyArg::Nexp(n);
            }
        }
    }
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
            // The CST also keeps `'n * 8` in TYPE_VAR. Only a bare
            // variable can be a polymorphic type argument; treating the whole
            // expression as a Param freshens it independently from `'n`.
            match NumericExpr::parse(text.trim()) {
                Some(n) if !matches!(n, NumericExpr::Var(_)) => TyArg::Nexp(n),
                _ => TyArg::Type(type_from_cst_node(node)),
            }
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
    let spec = node.parent().filter(|p| p.kind() == parser::SyntaxKind::CALLABLE_SPEC);
    let is_declared_pure = spec
        .as_ref()
        .unwrap_or(node)
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| t.kind() == parser::SyntaxKind::KW_PURE);

    // Collect forall quantifiers and constraints
    let body_node = collect_forall_from_cst(node, &mut quantifiers, &mut constraints);
    let inner = body_node.as_ref().unwrap_or(node);

    let parsed = type_from_cst_node(inner);
    let mut kind_bounds = kinds::declared(node);
    kinds::infer(&parsed, &mut kind_bounds);
    let parsed = kinds::normalize(&parsed, &kind_bounds);
    for c in &mut constraints {
        c.expr = kinds::normalize_constraint(&c.expr, &kind_bounds);
    }
    match parsed.kind() {
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
            let ret = ret.clone();
            // No flattening here: the `TYPE_ARROW` lowering already splits a
            // parenthesised parameter list into separate parameters, so
            // unwrapping a lone tuple again would turn `((int, int)) -> int`
            // — one tuple parameter — into a two-parameter function.
            let implicit_params: Vec<bool> = params.iter().map(ty_is_implicit).collect();
            let params = params.clone();
            TypeScheme {
                quantifiers,
                kind_bounds,
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
            kind_bounds,
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

    // The quantifier list is a `TYPE_PARAM_LIST` of `TYPE_PARAM` nodes and
    // the constraint is the type node between it and the body — upstream's
    // `typquant := kopt_list [',' typ]`, which the parser now builds.
    //
    // This was 90 lines of token heuristics: collect every direct token,
    // find the first comparison operator, walk back to the comma before it
    // to decide where the names stopped. It had to be, because `forall` was
    // parsed by bumping tokens until the `.`.
    let mut children = node.children();
    let param_list = children.next().filter(|n| n.kind() == SK::TYPE_PARAM_LIST);
    let rest: Vec<syntax::SyntaxNode> = children.collect();

    if let Some(param_list) = &param_list {
        for param in param_list.children().filter(|n| n.kind() == SK::TYPE_PARAM) {
            // `forall ('m : Int) ('o : Order).` — an Order-kinded variable
            // can never be bound, because `type_from_cst_node` drops the
            // order argument of `vector`/`bitvector` to match upstream's
            // typed AST (`type_env.ml`'s `builtin_typs`). Keeping it would
            // make every call to `bitwise_not : forall ('m : Int) ('o :
            // Order). vector('m, 'o, bit) -> ..` report `'o` unresolvable.
            if type_param_kind(&param).as_deref() == Some("Order") {
                continue;
            }
            _constraints.extend(natural_kind_constraints(&param));
            for name in type_param_names(&param) {
                if !quantifiers.contains(&name) {
                    quantifiers.push(name);
                }
            }
        }
    }

    // The constraint, when there is one, is the node before the body.
    if rest.len() >= 2 {
        if let Some(c) = quant_constraint_from_node(&rest[0]) {
            _constraints.push(c);
        }
    }

    // The body is the last child either way.
    node.children().last()
}

/// One `typquant` constraint, read off the type node that holds it.
fn quant_constraint_from_node(node: &syntax::SyntaxNode) -> Option<QuantConstraint> {
    use parser::SyntaxKind as SK;
    let tokens: Vec<(SK, String)> = node
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| !t.kind().is_trivia())
        .map(|t| (t.kind(), t.text().to_string()))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let raw_text: String = tokens.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" ");
    let text = raw_text.replace("{ ", "{").replace(" }", "}").replace(" , ", ", ");
    let mut mentions: Vec<String> =
        tokens.iter().filter(|(k, _)| *k == SK::TY_VAR).map(|(_, t)| t.clone()).collect();
    mentions.sort();
    mentions.dedup();
    let expr = parse_constraint_from_tokens(&tokens);
    Some(QuantConstraint { text, mentions, expr })
}

/// The `typquant` written on a definition rather than on a `val`.
///
/// `function f forall 'n, C . (args) -> ret = body` binds and constrains
/// exactly as a `val` spec does — upstream builds one `typquant` for both
/// (`parser.mly`'s `funcl_typ`). The parser used to bump the whole preamble
/// as loose tokens, so the scheme built from such a definition had no
/// quantifiers and no constraints: 33 definitions in sail-riscv, whose
/// bodies were checked without the facts written on their own heads.
///
/// Returns the quantifier names, the constraints, and where the preamble
/// ends, so the caller can find the return type *after* it — the constraint
/// is a type node too, and would otherwise be read as the signature.
pub(crate) fn callable_typquant(
    def_node: &syntax::SyntaxNode,
) -> (Vec<String>, Vec<QuantConstraint>, Option<syntax::TextSize>) {
    use parser::SyntaxKind as SK;
    let mut quantifiers = Vec::new();
    let mut constraints = Vec::new();

    let forall = def_node
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::KW_FORALL);
    let Some(forall) = forall else { return (quantifiers, constraints, None) };
    let dot = def_node
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::DOT && t.text_range().start() > forall.text_range().start());
    let Some(dot) = dot else { return (quantifiers, constraints, None) };
    let end = dot.text_range().end();

    let in_preamble = |n: &syntax::SyntaxNode| {
        n.text_range().start() > forall.text_range().start() && n.text_range().end() <= end
    };
    for child in def_node.children().filter(in_preamble) {
        match child.kind() {
            SK::TYPE_PARAM_LIST => {
                for param in child.children().filter(|n| n.kind() == SK::TYPE_PARAM) {
                    // An `Order`-kinded variable can never be bound — see
                    // `collect_forall_from_cst` for why.
                    if type_param_kind(&param).as_deref() == Some("Order") {
                        continue;
                    }
                    constraints.extend(natural_kind_constraints(&param));
                    for name in type_param_names(&param) {
                        if !quantifiers.contains(&name) {
                            quantifiers.push(name);
                        }
                    }
                }
            }
            k if k.is_type_node() => {
                if let Some(c) = quant_constraint_from_node(&child) {
                    constraints.push(c);
                }
            }
            _ => {}
        }
    }
    (quantifiers, constraints, Some(end))
}

/// Nat binders carry the same nonnegative predicate as upstream lowering.
fn natural_kind_constraints(param: &syntax::SyntaxNode) -> Vec<QuantConstraint> {
    if type_param_kind(param).as_deref() != Some("Nat") {
        return Vec::new();
    }
    // Upstream lowers Nat to Int together with a nonnegative constraint
    // (initial_check.ml: parse_kind_constraint).
    type_param_names(param)
        .into_iter()
        .map(|name| {
            let expr = ConstraintExpr::Compare {
                lhs: NumericExpr::Var(name.clone()),
                op: CompareOp::Gte,
                rhs: NumericExpr::Const(0),
            };
            QuantConstraint { text: expr.to_text(), mentions: vec![name], expr }
        })
        .collect()
}

/// The names one `kopt` binds. `('n 'm : Int)` binds two.
fn type_param_names(param: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    param
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::TY_VAR)
        .map(|t| t.text().to_string())
        .collect()
}

/// The kind written on a `kopt`, if one was. Sail defaults an unwritten
/// kind to `Int`, which is why this answers `None` rather than guessing.
fn type_param_kind(param: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    let mut toks = param.children_with_tokens().filter_map(|el| el.into_token());
    toks.find(|t| t.kind() == SK::COLON)?;
    toks.find(|t| !t.kind().is_trivia()).map(|t| t.text().to_string())
}

/// Parse a constraint expression from forall preamble tokens.
///
/// Handles:
/// - `'n in { 1 , 2 }` → InSet
/// - `0 < 'n <= 64` → And([Compare(0 < 'n), Compare('n <= 64)])
/// - `'n >= 0` → Compare('n >= 0)
fn parse_constraint_from_tokens(tokens: &[(parser::SyntaxKind, String)]) -> ConstraintExpr {
    use parser::SyntaxKind as SK;

    // Parenthesized predicates retain their scope when inserted as arguments.
    let meaningful: Vec<_> =
        tokens.iter().enumerate().filter(|(_, (k, _))| !k.is_trivia()).collect();
    if let (Some((start, (SK::L_PAREN, _))), Some((end, (SK::R_PAREN, _)))) =
        (meaningful.first(), meaningful.last())
    {
        let mut depth = 0;
        let encloses = meaningful.iter().all(|(i, (k, _))| {
            match k {
                SK::L_PAREN => depth += 1,
                SK::R_PAREN => depth -= 1,
                _ => {}
            }
            depth > 0 || i == end
        });
        if encloses {
            return parse_constraint_from_tokens(&tokens[start + 1..*end]);
        }
    }

    // 0. Split at `|` (disjunction) — lowest precedence (below `&`).
    // Only split at top-level `|` tokens (not inside parens/braces).
    let pipe_positions: Vec<usize> = {
        let mut depth = 0i32;
        tokens
            .iter()
            .enumerate()
            .filter_map(|(i, (k, _))| match k {
                SK::L_PAREN | SK::L_CURLY => {
                    depth += 1;
                    None
                }
                SK::R_PAREN | SK::R_CURLY => {
                    depth -= 1;
                    None
                }
                SK::PIPE if depth == 0 => Some(i),
                _ => None,
            })
            .collect()
    };
    if !pipe_positions.is_empty() {
        let mut parts: Vec<ConstraintExpr> = Vec::new();
        let mut start = 0;
        for &pos in &pipe_positions {
            let sub = parse_constraint_from_tokens(&tokens[start..pos]);
            parts.push(sub);
            start = pos + 1;
        }
        let sub = parse_constraint_from_tokens(&tokens[start..]);
        parts.push(sub);
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
            .filter_map(|(i, (k, _))| match k {
                SK::L_PAREN | SK::L_CURLY => {
                    depth += 1;
                    None
                }
                SK::R_PAREN | SK::R_CURLY => {
                    depth -= 1;
                    None
                }
                SK::AMP if depth == 0 => Some(i),
                _ => None,
            })
            .collect()
    };
    if !amp_positions.is_empty() {
        let mut parts: Vec<ConstraintExpr> = Vec::new();
        let mut start = 0;
        for &pos in &amp_positions {
            let sub = parse_constraint_from_tokens(&tokens[start..pos]);
            parts.push(sub);
            start = pos + 1;
        }
        let sub = parse_constraint_from_tokens(&tokens[start..]);
        parts.push(sub);
        if parts.len() > 1 {
            return ConstraintExpr::And(parts);
        } else if parts.len() == 1 {
            return parts.into_iter().next().unwrap();
        }
    }

    // Preserve negation in serialized constraint arguments as well as `not(...)`.
    if let Some((first, (_, _))) = meaningful.first().filter(|(_, (_, text))| text == "~") {
        return ConstraintExpr::Not(Box::new(parse_constraint_from_tokens(&tokens[first + 1..])));
    }

    // 2. Check for bare `true`/`false`.
    let meaningful: Vec<_> = tokens.iter().filter(|(k, _)| !k.is_trivia()).collect();
    if meaningful.len() == 1 {
        match meaningful[0].1.as_str() {
            "true" => return ConstraintExpr::Bool(true),
            "false" => return ConstraintExpr::Bool(false),
            _ if matches!(meaningful[0].0, SK::TY_VAR | SK::IDENT) => {
                return ConstraintExpr::BoolVar(meaningful[0].1.clone())
            }
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
            // Parse whole members; dropping punctuation changes -1 and 2^100.
            let start = rhs_tokens.iter().position(|(k, _)| *k == SK::L_CURLY);
            let end = rhs_tokens.iter().rposition(|(k, _)| *k == SK::R_CURLY);
            let items = match (start, end) {
                (Some(start), Some(end)) if start < end => {
                    let mut depth = 0;
                    let mut begin = start + 1;
                    let mut items = Vec::new();
                    for i in start + 1..=end {
                        let kind = rhs_tokens[i].0;
                        if kind == SK::L_PAREN {
                            depth += 1;
                        } else if kind == SK::R_PAREN {
                            depth -= 1;
                        }
                        if i == end || (kind == SK::COMMA && depth == 0) {
                            let Some(item) = numeric_expr_from_token_slice(&rhs_tokens[begin..i])
                            else {
                                return ConstraintExpr::Unsupported;
                            };
                            items.push(item);
                            begin = i + 1;
                        }
                    }
                    items
                }
                _ => Vec::new(),
            };
            if !items.is_empty() {
                return ConstraintExpr::InSet { value, items };
            }
        }
    }

    // 4. Check for chained comparison: `A op1 B op2 C` → And([Compare(A op1 B), Compare(B op2 C)])
    let mut nesting = 0i32;
    let cmp_positions: Vec<(usize, CompareOp)> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, (k, t))| {
            match k {
                SK::L_PAREN | SK::L_CURLY => nesting += 1,
                SK::R_PAREN | SK::R_CURLY => nesting -= 1,
                _ => {}
            }
            if nesting != 0 {
                return None;
            }
            match k {
                SK::L_ANGLE => Some((i, CompareOp::Lt)),
                SK::R_ANGLE => Some((i, CompareOp::Gt)),
                SK::LE => Some((i, CompareOp::Lte)),
                SK::GE => Some((i, CompareOp::Gte)),
                SK::EQ_EQ => Some((i, CompareOp::Eq)),
                _ if t == "!=" => Some((i, CompareOp::Neq)),
                _ => None,
            }
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
        let a = parse_constraint_from_tokens(lhs_slice);
        let b = parse_constraint_from_tokens(rhs_slice);
        if matches!(op, CompareOp::Eq | CompareOp::Neq)
            && (matches!(a, ConstraintExpr::Bool(_)) || matches!(b, ConstraintExpr::Bool(_)))
            && !matches!(a, ConstraintExpr::Unsupported)
            && !matches!(b, ConstraintExpr::Unsupported)
        {
            let eq = ConstraintExpr::Or(vec![
                ConstraintExpr::And(vec![a.clone(), b.clone()]),
                ConstraintExpr::And(vec![
                    ConstraintExpr::Not(Box::new(a)),
                    ConstraintExpr::Not(Box::new(b)),
                ]),
            ]);
            return if op == CompareOp::Eq { eq } else { ConstraintExpr::Not(Box::new(eq)) };
        }
        if let (Some(lhs), Some(rhs)) =
            (numeric_expr_from_token_slice(lhs_slice), numeric_expr_from_token_slice(rhs_slice))
        {
            return ConstraintExpr::Compare { lhs, op, rhs };
        }
    }

    // `is_mem_width('width)` — upstream's `NC_app`.
    if tokens.len() >= 3
        && tokens[0].0 == SK::IDENT
        && tokens[1].0 == SK::L_PAREN
        && tokens.last().is_some_and(|(k, _)| *k == SK::R_PAREN)
    {
        let inner = &tokens[2..tokens.len() - 1];
        if inner.iter().all(|(k, _)| k.is_trivia()) {
            return ConstraintExpr::App { name: tokens[0].1.clone(), args: Vec::new() };
        }
        let mut args = Vec::new();
        let mut ok = true;
        let mut start = 0;
        let mut depth = 0;
        for end in 0..=inner.len() {
            if end < inner.len() {
                match inner[end].0 {
                    SK::L_PAREN | SK::L_CURLY => depth += 1,
                    SK::R_PAREN | SK::R_CURLY => depth -= 1,
                    _ => {}
                }
            }
            if end == inner.len() || (depth == 0 && inner[end].0 == SK::COMMA) {
                let part = &inner[start..end];
                start = end + 1;
                let boolean = parse_constraint_from_tokens(part);
                let numeric = numeric_expr_from_token_slice(part);
                let arg = match (&boolean, numeric) {
                    (
                        ConstraintExpr::Bool(_)
                        | ConstraintExpr::Compare { .. }
                        | ConstraintExpr::And(_)
                        | ConstraintExpr::Or(_)
                        | ConstraintExpr::Not(_),
                        _,
                    ) => TyArg::Bool(boolean),
                    (_, Some(n)) => TyArg::Nexp(n),
                    (ConstraintExpr::Unsupported, None) => {
                        ok = false;
                        break;
                    }
                    (_, None) => TyArg::Bool(boolean),
                };
                args.push(arg);
            }
        }
        if ok && !args.is_empty() {
            return ConstraintExpr::App { name: tokens[0].1.clone(), args };
        }
    }

    ConstraintExpr::Unsupported
}

/// Parse a constraint expression from a plain text string.
///
/// Tokenizes the text using a simple character scanner and delegates to
/// `parse_constraint_from_tokens`. Used for `NumericExpr::If` conditions
/// which are parsed as text by `NumericTextParser`.
/// Re-read any numeric operand the token walk left as a bare `Symbol`.
///
/// It joins the tokens it did not understand back into a string —
/// `div ( 'm , 2 )` — and a symbol substitutes into nothing and folds to
/// nothing, so `'n == div('m, 2)` determined `'n` for no one.
/// `NumericExpr::parse` reads the whole grammar now.
pub(super) fn renormalise_nexp(e: &NumericExpr) -> NumericExpr {
    match e {
        NumericExpr::Symbol(s) if !s.chars().all(|c| c.is_alphanumeric() || c == '_') => {
            NumericExpr::parse(s).unwrap_or_else(|| e.clone())
        }
        _ => e.clone(),
    }
}

pub(super) fn renormalise_constraint(e: ConstraintExpr) -> ConstraintExpr {
    match e {
        ConstraintExpr::Compare { lhs, op, rhs } => {
            ConstraintExpr::Compare { lhs: renormalise_nexp(&lhs), op, rhs: renormalise_nexp(&rhs) }
        }
        ConstraintExpr::InSet { value, items } => ConstraintExpr::InSet {
            value: renormalise_nexp(&value),
            items: items.iter().map(renormalise_nexp).collect(),
        },
        ConstraintExpr::And(items) => {
            ConstraintExpr::And(items.into_iter().map(renormalise_constraint).collect())
        }
        ConstraintExpr::Or(items) => {
            ConstraintExpr::Or(items.into_iter().map(renormalise_constraint).collect())
        }
        ConstraintExpr::Not(inner) => ConstraintExpr::Not(Box::new(renormalise_constraint(*inner))),
        other => other,
    }
}

impl QuantConstraint {
    /// The largest value each quantifier this constraint bounds can take.
    ///
    /// `'n in {2, 3}` gives 3; `'n <= 8` gives 8; `'n < 8` gives 7. Only a
    /// bound that is a constant — anything else is the solver's.
    pub(super) fn quantifier_upper_bounds(&self) -> Vec<(String, i128)> {
        fn walk(e: &ConstraintExpr, out: &mut Vec<(String, i128)>) {
            match e {
                ConstraintExpr::InSet { value: NumericExpr::Var(v), items } => {
                    let mut best: Option<i64> = None;
                    for i in items {
                        match i {
                            NumericExpr::Const(n) => best = Some(best.map_or(*n, |b| b.max(*n))),
                            _ => return,
                        }
                    }
                    if let Some(b) = best {
                        out.push((v.clone(), b as i128));
                    }
                }
                ConstraintExpr::Compare { lhs: NumericExpr::Var(v), op, rhs } => {
                    let Some(k) = crate::infer::nexp_simplify::try_eval_const(rhs) else { return };
                    match op {
                        CompareOp::Lte => out.push((v.clone(), k as i128)),
                        CompareOp::Lt => out.push((v.clone(), k as i128 - 1)),
                        CompareOp::Eq => out.push((v.clone(), k as i128)),
                        _ => {}
                    }
                }
                ConstraintExpr::And(items) => items.iter().for_each(|i| walk(i, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&self.expr, &mut out);
        out
    }
}

pub(super) fn parse_constraint_text(text: &str) -> ConstraintExpr {
    renormalise_constraint(parse_constraint_text_inner(text))
}

fn parse_constraint_text_inner(text: &str) -> ConstraintExpr {
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
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '#')) {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push((SK::TY_VAR, s));
            continue;
        }
        // Number
        if chars[i].is_ascii_digit()
            || (chars[i] == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
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
                "<=" => {
                    tokens.push((SK::LE, two));
                    i += 2;
                    continue;
                }
                ">=" => {
                    tokens.push((SK::GE, two));
                    i += 2;
                    continue;
                }
                "==" => {
                    tokens.push((SK::EQ_EQ, two));
                    i += 2;
                    continue;
                }
                "!=" => {
                    tokens.push((SK::NEQ, "!=".to_string()));
                    i += 2;
                    continue;
                }
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
            '~' => SK::OP_IDENT,
            _ => SK::ERROR,
        };
        tokens.push((kind, chars[i].to_string()));
        i += 1;
    }

    parse_constraint_from_tokens(&tokens)
}

/// Parse a numeric expression from a slice of tokens.
fn numeric_expr_from_token_slice(tokens: &[(parser::SyntaxKind, String)]) -> Option<NumericExpr> {
    let text = tokens.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join(" ");
    NumericExpr::parse(&text)
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
                .find(|t| matches!(t.kind(), SK::IDENT | SK::STRING_LIT))
                // `val "shift_bits_right" : ...` names the function with a
                // string literal (parser.mly `val_spec`); the declared name
                // is the string's contents.
                .map(|t| t.text().trim_matches('"').to_string());
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
    // Only outer parameter annotations contribute to the function type.
    // An `as int('n)` inside a constructor introduces a type-level binder;
    // it is not another parameter or the constructor's argument type.
    param_list
        .descendants()
        .filter(|n| n != &param_list && n.kind().is_type_node())
        // Only take top-level type nodes (not nested children of other type nodes)
        .filter(|n| {
            // Flat parameter syntax and parsed outer typed patterns coexist.
            n.parent().is_some_and(|parent| {
                parent == param_list
                    || (parent.kind() == parser::SyntaxKind::TYPED_PAT
                        && parent.parent().as_ref() == Some(&param_list))
            })
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

// Parse a constraint expression from source text (for assert propagation).

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
///
/// The declaration is read by `hir_def::item_tree::extract_field_decls`, which
/// the item tree already runs over the same node; this only turns each field's
/// declared text into a `Ty`.
///
/// It used to be a second reader with its own token walk, and the two
/// disagreed wherever the field list nests. That walk tracked one `in_braces`
/// flag and no depth, so the inner `}` of
/// `Three : (int, {'n, 'n in {16, 32}. (int('n), bits('n))})` closed the
/// struct as far as it was concerned: every field after it was invisible, and
/// `st1.Three` reported "no field `Three` on type `st`" on a file upstream
/// accepts (`sail/test/rocq/pass/existentials2.sail`). The item tree's reader
/// counts brace depth and paren group, so a comma inside a field's type is not
/// a separator.
fn extract_struct_fields_from_cst(node: &syntax::SyntaxNode) -> HashMap<String, Ty> {
    hir_def::item_tree::extract_field_decls(node)
        .into_iter()
        .map(|f| {
            let ty = if f.type_text.is_empty() {
                Ty::error()
            } else {
                type_from_type_text(&f.type_text)
            };
            (f.name.as_str().to_string(), ty)
        })
        .collect()
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
/// Enum member names, in either spelling.
///
/// `enum E = { A, B }` and `enum E = A | B` are both accepted by upstream
/// (parser.mly `enum_bar`). Keeping these as two functions meant every
/// caller had to remember to try both, and the ones that forgot treated a
/// bar-separated member as a variable binding.
fn extract_enum_members(node: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    let braced = extract_braced_idents(node);
    if !braced.is_empty() {
        return braced;
    }
    let mut out = Vec::new();
    let mut seen_eq = false;
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::EQ if !seen_eq => seen_eq = true,
            SK::IDENT if seen_eq => out.push(tok.text().to_string()),
            _ => {}
        }
    }
    out
}

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
    let underlying = find_type_child(node).map(|n| type_from_cst_node(&n))?;
    let fields = hir_def::bitfield::field_widths(node)
        .into_iter()
        .map(|(name, width)| {
            let ty = NumericExpr::parse(&width)
                .map(|width| {
                    let width = nexp_simplify::simplify(&width);
                    if width == NumericExpr::Const(1) {
                        Ty::named("bit")
                    } else {
                        Ty::app("bits", vec![TyArg::Nexp(width)], "")
                    }
                })
                .unwrap_or_else(Ty::error);
            (name, ty)
        })
        .collect();
    Some(BitfieldInfo { underlying, fields })
}

/// Extract union variants from CST: `{ Name : Type, ... }`.
fn extract_union_variants_from_cst(node: &syntax::SyntaxNode) -> Vec<(String, Ty)> {
    use parser::SyntaxKind as SK;
    let mut variants = Vec::new();
    // Brace depth, not a boolean: a constructor payload can itself be an
    // existential `{'d, constraint. typ}`, whose braces and commas must not
    // be mistaken for the end of the variant list. Treating them as such lost
    // every constructor after the first existential one.
    let mut depth = 0u32;
    // Parenthesis/bracket depth *inside* a payload. A tuple payload
    // `HART_WAITING : (WaitReason, instbits)` contains a comma that is not a
    // variant separator; without this the second element's type name became a
    // third variant, and the exhaustiveness checker then demanded an arm for
    // a constructor that does not exist.
    let mut group_depth = 0u32;
    let mut current_name: Option<String> = None;
    // Set once a variant's payload type has been consumed, so identifiers
    // nested inside it are not mistaken for the next variant name. Cleared
    // at the comma that starts the next variant.
    let mut type_consumed = false;

    let mut it = node.descendants_with_tokens().peekable();
    while let Some(el) = it.next() {
        // Attributes such as `$[extension B] Baz : unit` carry identifiers
        // that are not variant names.
        if let rowan::NodeOrToken::Node(ref n) = el {
            if ast::Attribute::can_cast(n.kind()) {
                let range = n.text_range();
                while it.peek().is_some_and(|next| range.contains_range(next.text_range())) {
                    it.next();
                }
                continue;
            }
        }
        match el {
            rowan::NodeOrToken::Token(tok) => match tok.kind() {
                // A union body is lowered as flat tokens, so an inline
                // `$[extension B]` attribute arrives as `$ [ ... ]` rather
                // than an `ATTRIBUTE` node. Skip to the closing bracket so
                // its identifiers are not read as constructor names.
                SK::DOLLAR => {
                    while let Some(next) = it.peek() {
                        let done = next.as_token().is_some_and(|t| t.kind() == SK::R_BRACK);
                        it.next();
                        if done {
                            break;
                        }
                    }
                }
                SK::L_PAREN | SK::L_BRACK => group_depth += 1,
                SK::R_PAREN | SK::R_BRACK => group_depth = group_depth.saturating_sub(1),
                SK::L_CURLY => depth += 1,
                SK::R_CURLY => {
                    if depth == 1 {
                        if let Some(name) = current_name.take() {
                            variants.push((name, Ty::named("unit")));
                        }
                        type_consumed = false;
                    }
                    depth = depth.saturating_sub(1);
                }
                SK::COMMA if depth == 1 && group_depth == 0 => {
                    if let Some(name) = current_name.take() {
                        // No type annotation — default to unit.
                        variants.push((name, Ty::named("unit")));
                    }
                    type_consumed = false;
                }
                SK::IDENT
                    if depth == 1
                        && group_depth == 0
                        && current_name.is_none()
                        && !type_consumed =>
                {
                    current_name = Some(tok.text().to_string());
                }
                SK::TY_VAR if depth >= 1 && current_name.is_some() => {
                    // Type variable as variant payload: `Some : 'a`
                    if let Some(name) = current_name.take() {
                        variants.push((name, Ty::param(tok.text().to_string())));
                        type_consumed = true;
                    }
                }
                SK::IDENT if depth == 1 && current_name.is_some() => {
                    // Second IDENT after `:` — this is the type name.
                    if let Some(name) = current_name.take() {
                        variants.push((name, Ty::named(tok.text().to_string())));
                        type_consumed = true;
                    }
                }
                _ => {}
            },
            rowan::NodeOrToken::Node(n) if depth >= 1 && current_name.is_some() => {
                let is_type = matches!(
                    n.kind(),
                    SK::TYPE_NAMED
                        | SK::TYPE_VAR
                        | SK::TYPE_APP
                        | SK::TYPE_TUPLE
                        | SK::TYPE_ARROW
                        | SK::TYPE_FORALL
                        | SK::TYPE_EXISTENTIAL
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
        TyKind::Named(_name, _) => false, // ADTs might be records — we don't know without lookup
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
    workspace_known_constructors: bool,
) -> bool {
    if pattern_constants.contains(name) {
        return false;
    }
    if PRELUDE_CONSTRUCTORS.contains(&name) {
        return false;
    }
    // Sail has no case convention: `float_class_negative_inf` is an enum
    // member and `Foo` may be a variable. When the whole workspace has been
    // seen, `pattern_constants` already holds every constructor and enum
    // member, so anything left really is a binding. Only without it does the
    // usual capitalisation have to stand in.
    if !workspace_known_constructors && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
    {
        return false;
    }
    true
}

/// `{'n, lo <= 'n & 'n <= hi. atom('n)}` as `range(lo, hi)`.
///
/// `None` unless there is exactly one bound variable, the body is that
/// variable's `atom`, and the constraint gives it a literal lower and upper
/// bound. Anything looser stays an existential — a witness this cannot see
/// is not a range.
fn existential_as_range(vars: &[String], constraint: &ConstraintExpr, inner: &Ty) -> Option<Ty> {
    use crate::ty::{CompareOp, TyArg, TyKind};
    let [var] = vars else { return None };
    let TyKind::App { name, args, .. } = inner.kind() else { return None };
    // The body is the bound variable itself, as `atom('n)` or `int('n)`. It
    // reaches here either as a numeric argument or as a `Param` type.
    if !matches!(name.as_str(), "atom" | "int") || args.len() != 1 {
        return None;
    }
    let body_var = match args.first()? {
        TyArg::Nexp(n) => n.to_string_repr(),
        TyArg::Bool(c) => c.to_text(),
        TyArg::Order(v) | TyArg::Value(v) => v.clone(),
        TyArg::Type(t) => match t.kind() {
            TyKind::Param(p) => p.clone(),
            _ => return None,
        },
    };
    if body_var.trim() != var.as_str() {
        return None;
    }

    // This conversion must be an equivalence, not merely an interval
    // approximation: consumers can use the result to refute constraints.
    fn exact_interval(c: &ConstraintExpr, var: &str) -> bool {
        match c {
            ConstraintExpr::And(parts) => parts.iter().all(|c| exact_interval(c, var)),
            ConstraintExpr::Compare { lhs, op, rhs } => {
                *op != CompareOp::Neq
                    && ((lhs.to_string_repr() == var
                        && rhs.to_string_repr().parse::<i128>().is_ok())
                        || (rhs.to_string_repr() == var
                            && lhs.to_string_repr().parse::<i128>().is_ok()))
            }
            ConstraintExpr::InSet { value, items } if value.to_string_repr() == var => {
                let Some(mut values) = items
                    .iter()
                    .map(|n| n.to_string_repr().parse::<i128>().ok())
                    .collect::<Option<Vec<_>>>()
                else {
                    return false;
                };
                values.sort();
                values.dedup();
                !values.is_empty() && values.windows(2).all(|w| w[0].checked_add(1) == Some(w[1]))
            }
            _ => false,
        }
    }
    if !exact_interval(constraint, var) {
        return None;
    }

    let mut lo: Option<i128> = None;
    let mut hi: Option<i128> = None;
    fn walk(c: &ConstraintExpr, var: &str, lo: &mut Option<i128>, hi: &mut Option<i128>) {
        let ConstraintExpr::Compare { lhs, op, rhs } = c else { return };
        let (l, r) = (lhs.to_string_repr(), rhs.to_string_repr());
        let (num, var_side, op) = if l.trim() == var {
            (r, true, *op)
        } else if r.trim() == var {
            (l, false, *op)
        } else {
            return;
        };
        let Ok(n) = num.trim().parse::<i128>() else { return };
        // `'n <= 4` and `4 >= 'n` are the same bound.
        let effective = if var_side {
            op
        } else {
            match op {
                CompareOp::Lt => CompareOp::Gt,
                CompareOp::Lte => CompareOp::Gte,
                CompareOp::Gt => CompareOp::Lt,
                CompareOp::Gte => CompareOp::Lte,
                other => other,
            }
        };
        match effective {
            CompareOp::Lte => *hi = Some(hi.map_or(n, |h: i128| h.min(n))),
            CompareOp::Lt => *hi = Some(hi.map_or(n - 1, |h: i128| h.min(n - 1))),
            CompareOp::Gte => *lo = Some(lo.map_or(n, |l: i128| l.max(n))),
            CompareOp::Gt => *lo = Some(lo.map_or(n + 1, |l: i128| l.max(n + 1))),
            CompareOp::Eq => {
                *lo = Some(n);
                *hi = Some(n);
            }
            CompareOp::Neq => {}
        }
    }
    // `'n in {8, 16, 32, 64}` bounds the variable too — by its extremes.
    // A widening for a non-contiguous set, so it can only accept too much.
    fn from_set(
        c: &ConstraintExpr,
        var: &str,
        lo: &mut Option<i128>,
        hi: &mut Option<i128>,
    ) -> bool {
        let ConstraintExpr::InSet { value, items } = c else { return false };
        if value.to_string_repr().trim() != var {
            return false;
        }
        let nums: Vec<i128> =
            items.iter().filter_map(|i| i.to_string_repr().trim().parse().ok()).collect();
        if nums.len() != items.len() || nums.is_empty() {
            return false;
        }
        *lo = Some(*nums.iter().min().unwrap());
        *hi = Some(*nums.iter().max().unwrap());
        true
    }
    match constraint {
        ConstraintExpr::And(parts) => {
            for part in parts {
                if !from_set(part, var, &mut lo, &mut hi) {
                    walk(part, var, &mut lo, &mut hi);
                }
            }
        }
        other => {
            if !from_set(other, var, &mut lo, &mut hi) {
                walk(other, var, &mut lo, &mut hi)
            }
        }
    }
    let (lo, hi) = (lo?, hi?);
    if lo > hi {
        return None;
    }
    Some(Ty::app(
        "range",
        vec![TyArg::numeric(lo.to_string()), TyArg::numeric(hi.to_string())],
        format!("range({lo}, {hi})"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cst_env(source: &str) -> TopLevelEnv {
        let (root, _) = syntax::parse_text(source);
        TopLevelEnv::from_cst(&root).0
    }

    /// A struct's field list is read once, and the item tree owns the reader.
    ///
    /// Both layers need the same fact — the item tree to say what the file
    /// declares, `RecordInfo` to type a field access — and each had its own
    /// token walk. Only one of them counted brace depth, so they disagreed
    /// on exactly the declarations where it matters: a field whose type
    /// nests braces (`{'n, 'n in {16, 32}. ..}`) ended the field list for
    /// the other, and every field after it was invisible.
    ///
    /// Delegating is the invariant, not the current implementation of it —
    /// so this fails if `extract_struct_fields_from_cst` starts naming
    /// syntax kinds again.
    #[test]
    fn a_struct_field_list_has_one_reader() {
        const THIS: &str = include_str!("mod.rs");
        let body = THIS
            .split("fn extract_struct_fields_from_cst")
            .nth(1)
            .expect("extract_struct_fields_from_cst is gone")
            .split("\nfn ")
            .next()
            .unwrap();
        assert!(
            body.contains("hir_def::item_tree::extract_field_decls"),
            "hir-ty reads a struct's field list itself instead of asking the item tree"
        );
        assert!(
            !body.contains("SK::"),
            "hir-ty is walking tokens to find field names again; the item tree's \
             `extract_field_decls` counts brace depth and paren group and this did not"
        );
    }

    /// The field list ends at the brace that opened it, not at the first `}`.
    #[test]
    fn a_nested_field_type_does_not_end_the_field_list() {
        let env = cst_env(
            "struct st = {\n  One : (int, int),\n  \
             Two : {'n, 'n in {16, 32}. (int('n), bits('n))},\n  \
             Three : (int, {'n, 'n in {16, 32}. (int('n), bits('n))}),\n}\n",
        );
        let record = env.records.get("st").expect("struct st is not in the env");
        let mut names: Vec<&str> = record.fields.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["One", "Three", "Two"]);
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
        // The explicit `add` and both finite-domain enum conversions.
        assert_eq!(cst.functions.len(), 3, "functions");
        assert!(cst.functions.contains_key("color_of_num"));
        assert!(cst.functions.contains_key("num_of_color"));
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

#[cfg(test)]
mod p2_audit;
#[cfg(test)]
mod semantic_audit;

#[cfg(test)]
mod boolean_audit;

#[cfg(test)]
mod numeric_audit;

mod bounded_arithmetic;
#[cfg(test)]
mod mapping_width_audit;

#[cfg(test)]
mod exception_branch_audit;

#[cfg(test)]
mod unsupported_audit;
