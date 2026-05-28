//! Pattern exhaustiveness and redundancy checking.
//!
//! This is a Maranget-style matrix algorithm (the "useful" predicate from
//! Luc Maranget's 2008 "Warnings for pattern matching" paper) adapted for
//! Sail's pattern subset. We mirror the design rust-analyzer uses around
//! `rustc_pattern_analysis`: a separate post-inference pass with a clean
//! `Cx`-style trait so the algorithm itself stays language-agnostic.
//!
//! Compared to upstream Sail's `pattern_completeness.ml` we deliberately
//! handle a much smaller language:
//!
//! - **Wild / binding** — bare identifier-as-binding is treated as wildcard.
//! - **Constructor** — `Some(x)`, `Privilege::Machine`, etc. Both Sail's
//!   `App` patterns and bare uppercase `Ident` patterns lower to this.
//! - **Tuple** — `(x, y)` against `(_, A | B)`.
//! - **Or** — `A | B` patterns inside an arm.
//! - **Literal** — bool/number/string/bits/hex literals. The integer
//!   universe is treated as infinite, so a literal-only match is never
//!   certified as exhaustive (matching upstream's conservative posture).
//! - **List** — `[||]` (nil) and `h :: t` (cons) pairs, as in upstream
//!   Sail's `GP_empty_list` / `GP_cons`.
//! - **Vector** — fixed-length vector patterns `[p1, p2, ...]`. Rows whose
//!   vectors have different lengths never specialize against each other.
//! - **Struct** — `struct { a = pa, b = pb }` patterns, canonicalised by
//!   sorting field names alphabetically.
//! - **Typed / AsType / AsBinding / Attribute** — recurse through the
//!   wrapper to the inner pattern.
//!
//! Patterns we don't yet model (vector subrange, arbitrary infix) lower to
//! wildcard so they're treated as covering everything — conservative, no
//! false positives, no false negatives on the supported subset.
//!
//! Guards make completeness undecidable, so a guarded arm doesn't
//! contribute to exhaustiveness coverage (matches both upstream Sail and
//! rust-analyzer).

mod pat_analysis;
mod pat_util;

use std::collections::HashMap;

use crate::Span;
use parser::Literal;

// Re-export the driver from pat_analysis.
pub use pat_analysis::compute_match_usefulness;

/// Lightweight pattern form used by the matrix algorithm. Lowered from
/// `core_ast::Pattern` via `lower_pattern`.
#[derive(Clone, Debug)]
pub enum MatchPat {
    /// `_`, bare lowercase identifier, or any pattern we don't model.
    Wild,
    /// Constructor application: `Some(x)`, `None`, `Privilege::Machine`.
    /// `name` is the constructor name; `args` are the (possibly nested)
    /// sub-patterns. Nullary constructors have an empty `args` vector.
    Ctor { name: String, args: Vec<MatchPat> },
    /// `(p1, p2, ...)`.
    Tuple(Vec<MatchPat>),
    /// Numeric/string/bits/hex/bool literal.
    Literal(LiteralKey),
    /// `p1 | p2 | ...`. Or-patterns are expanded into multiple matrix
    /// rows during lowering, so the algorithm itself never sees this
    /// variant directly.
    Or(Vec<MatchPat>),
    /// Empty list `[||]`.
    Nil,
    /// `h :: t` — list cons cell.
    Cons(Box<MatchPat>, Box<MatchPat>),
    /// Fixed-length vector literal pattern `[p1, p2, ...]`. Rows with
    /// different lengths never specialize against each other.
    Vec(Vec<MatchPat>),
    /// Struct pattern. `fields` are stored sorted by name so that two
    /// rows with the same field set line up columnwise. Fields omitted by
    /// a row are elaborated to `Wild`.
    Struct { fields: Vec<(String, MatchPat)> },
    /// Vector concatenation pattern (`x[7..0] @ x[15..8]`).
    ///
    /// Represents a bitvector pattern with a known total width.
    /// During exhaustiveness checking, this is treated as equivalent to
    /// a wildcard of the same bit width — the subrange structure doesn't
    /// affect exhaustiveness (any pattern matching the full width covers it).
    ///
    /// `width` is the total number of bits covered. When two VectorConcat
    /// patterns have the same width, they cover the same space.
    VectorConcat { width: usize },
}

/// Hashable, equatable form of a literal pattern. We don't try to evaluate
/// integer ranges; the algorithm treats the universe of literals of a
/// given kind as infinite.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum LiteralKey {
    Bool(bool),
    Unit,
    Number(String),
    String(String),
    Binary(String),
    Hex(String),
    Undefined,
}

/// Type abstraction passed to the algorithm. The typechecker fills this
/// in with whatever shape it inferred for the scrutinee.
#[derive(Clone, Debug)]
pub enum MatchTy {
    /// Named enum, union or struct shell. Carries the type arguments at
    /// this instantiation so a generic union like `opt('a)` matched
    /// against `opt(opt(bool))` produces nested
    /// `Named("opt", [Named("opt", [Bool])])`, and the matrix's
    /// constructor specialization substitutes precisely at every
    /// recursion level.
    Named(String, Vec<MatchTy>),
    /// Tuple of N positional types.
    Tuple(Vec<MatchTy>),
    /// Bool — closed universe of `{ true, false }`.
    Bool,
    /// `list(elem)`. Closed universe of `{ Nil, Cons(elem, list(elem)) }`.
    List(Box<MatchTy>),
    /// `vector('n, elem)` — element type known, length typically is not.
    /// Treated as having an unlistable universe: a wildcard arm is
    /// required to certify exhaustiveness.
    Vector(Box<MatchTy>),
    /// Named record / struct. Has a single "constructor" — the struct
    /// itself — with one sub-position per field, sorted by field name.
    Record(String),
    /// `bits(N)` — bitvector of known width. For small widths (N <= 16),
    /// the exhaustiveness checker can enumerate all 2^N values and find
    /// a concrete missing witness. For larger widths, treated as
    /// `Unlistable` (requires a wildcard arm).
    Bits(usize),
    /// Type we couldn't classify. Treated as having an unlistable
    /// constructor set, so the algorithm only certifies a match
    /// exhaustive when a wildcard arm is present.
    Unknown,
}

/// Constructor universe for a given type. The algorithm uses this to
/// decide whether a wildcard arm is required for exhaustiveness.
#[derive(Clone, Debug)]
pub enum CtorSet {
    /// Closed set of constructors, each with a fixed arity. The algorithm
    /// can list missing constructors precisely.
    Closed(Vec<CtorInfo>),
    /// Type has a constructor universe but we don't know its members
    /// (e.g. cross-file enum we couldn't aggregate). Conservatively treat
    /// as unlistable.
    Unknown,
    /// Universe is infinite (numeric literals, strings). A wildcard is
    /// the only way to be exhaustive.
    Unlistable,
}

#[derive(Clone, Debug)]
pub struct CtorInfo {
    pub name: String,
    pub arity: usize,
}

/// Context the algorithm queries to enumerate constructors.
pub trait Cx {
    /// Return the constructor set for `ty`.
    fn ctors_for(&self, ty: &MatchTy) -> CtorSet;
    /// For a constructor named `name` appearing in a pattern, look up the
    /// types of its sub-patterns. Used to specialise the matrix when we
    /// step into a constructor row.
    fn ctor_sub_tys(&self, name: &str, scrutinee: &MatchTy) -> Vec<MatchTy>;
    /// For a struct scrutinee, return the types of the listed fields in
    /// the same order. Unknown fields map to `MatchTy::Unknown`.
    fn record_field_tys(&self, scrutinee: &MatchTy, fields: &[String]) -> Vec<MatchTy>;
    /// Return the canonical (sorted-by-name) field list for a record type
    /// plus each field's type. Used to construct missing-witness rows on
    /// struct columns. Empty vec means we couldn't classify the record.
    fn record_all_fields(&self, scrutinee: &MatchTy) -> Vec<(String, MatchTy)>;
}

/// One row of the match matrix. Each row holds the lowered pattern of
/// one (un-guarded) arm plus its source location for diagnostic ranges.
#[derive(Clone, Debug)]
pub struct Row {
    pub pats: Vec<MatchPat>,
    pub arm_span: Span,
    pub has_guard: bool,
}

/// Per-arm input to the algorithm.
#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: MatchPat,
    pub guard_span: Option<Span>,
    pub arm_span: Span,
}

/// Result of an exhaustiveness analysis.
#[derive(Clone, Debug, Default)]
pub struct UsefulnessReport {
    /// Witnesses (concrete patterns) that aren't covered by any arm.
    /// Empty if the match is exhaustive.
    pub missing_witnesses: Vec<MatchPat>,
    /// Arms whose pattern was completely subsumed by an earlier arm.
    pub redundant: Vec<Span>,
}

/// Lower a `Pat` from a Body arena to a `MatchPat`. Walks `Pat` children
/// arena-native equivalent of `lower_pattern` — walks `Pat` children
/// via `PatId` lookups in the Body instead of `core_ast::Pattern` Box
/// pointers.
///
/// Added in so exhaustiveness checking can work on the arena-
/// allocated HIR representation without going back to `core_ast`.
pub fn lower_pattern_hir(
    body: &hir_def::body::Body,
    pat_id: hir_def::body::PatId,
    pattern_constants: &impl PatternConstants,
) -> MatchPat {
    use hir_def::hir::Pat;
    let pat = match body.pat(pat_id) {
        Some(p) => p,
        None => return MatchPat::Wild,
    };
    match pat {
        Pat::Wild | Pat::Missing => MatchPat::Wild,
        Pat::Bind(name) => {
            if pattern_constants.contains(name) {
                MatchPat::Ctor { name: name.clone(), args: Vec::new() }
            } else {
                MatchPat::Wild
            }
        }
        Pat::Literal(lit) => MatchPat::Literal(literal_key(lit)),
        Pat::Tuple(items) => MatchPat::Tuple(
            items.iter().map(|&id| lower_pattern_hir(body, id, pattern_constants)).collect(),
        ),
        Pat::App { ctor, args } => MatchPat::Ctor {
            name: ctor.clone(),
            args: args.iter().map(|&id| lower_pattern_hir(body, id, pattern_constants)).collect(),
        },
        Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
            lower_pattern_hir(body, *inner, pattern_constants)
        }
        Pat::As { pat, .. } => lower_pattern_hir(body, *pat, pattern_constants),
        Pat::Infix { lhs, op, rhs } if op == "|" => MatchPat::Or(vec![
            lower_pattern_hir(body, *lhs, pattern_constants),
            lower_pattern_hir(body, *rhs, pattern_constants),
        ]),
        Pat::Infix { lhs, op, rhs } if op == "::" => MatchPat::Cons(
            Box::new(lower_pattern_hir(body, *lhs, pattern_constants)),
            Box::new(lower_pattern_hir(body, *rhs, pattern_constants)),
        ),
        Pat::List(items) => {
            let mut acc = MatchPat::Nil;
            for &id in items.iter().rev() {
                let head = lower_pattern_hir(body, id, pattern_constants);
                acc = MatchPat::Cons(Box::new(head), Box::new(acc));
            }
            acc
        }
        Pat::Array(items) => MatchPat::Vec(
            items.iter().map(|&id| lower_pattern_hir(body, id, pattern_constants)).collect(),
        ),
        Pat::Struct { fields, .. } => {
            let mut lowered: Vec<(String, MatchPat)> = fields
                .iter()
                .map(|(name, id)| (name.clone(), lower_pattern_hir(body, *id, pattern_constants)))
                .collect();
            lowered.sort_by(|a, b| a.0.cmp(&b.0));
            MatchPat::Struct { fields: lowered }
        }
        // Patterns we don't model exhaustively yet
        Pat::TypeVar(_) | Pat::Index { .. } | Pat::RangeIndex { .. } | Pat::Infix { .. } => {
            MatchPat::Wild
        }
    }
}

fn literal_key(lit: &Literal) -> LiteralKey {
    match lit {
        Literal::Bool(b) => LiteralKey::Bool(*b),
        Literal::Unit => LiteralKey::Unit,
        Literal::Number(s) => LiteralKey::Number(s.clone()),
        Literal::String(s) => LiteralKey::String(s.clone()),
        Literal::Binary(s) => LiteralKey::Binary(s.clone()),
        Literal::Hex(s) => LiteralKey::Hex(s.clone()),
        Literal::BitZero => LiteralKey::Binary("0b0".to_string()),
        Literal::BitOne => LiteralKey::Binary("0b1".to_string()),
        Literal::Undefined => LiteralKey::Undefined,
    }
}

/// Trait the lowerer uses to ask "is this bare-name pattern actually a
/// constructor?". Concrete typechecker passes a closure or its own set.
pub trait PatternConstants {
    fn contains(&self, name: &str) -> bool;
}

impl<F> PatternConstants for F
where
    F: Fn(&str) -> bool,
{
    fn contains(&self, name: &str) -> bool {
        (self)(name)
    }
}

/// Lower match arms from Expr's arena-native MatchArm representation.
/// Uses `lower_pattern_hir` (Body arena) instead of `lower_pattern` (core_ast).
pub fn lower_arms_hir<P>(
    body: &hir_def::body::Body,
    arms: &[hir_def::hir::MatchArm],
    pattern_constants: &P,
) -> Vec<Arm>
where
    P: PatternConstants,
{
    arms.iter()
        .map(|arm| {
            let pat = lower_pattern_hir(body, arm.pat, pattern_constants);
            // Spans are not available from Body alone after migration.
            // Use placeholder spans; proper fix is to pass BodySourceMap.
            let guard_span = arm.guard.map(|_| Span::new(0, 0));
            let arm_span = Span::new(0, 0);
            Arm { pat, guard_span, arm_span }
        })
        .collect()
}

/// A single record's canonical (sorted-by-name) field list with types.
#[derive(Clone, Debug)]
pub struct RecordFields {
    pub fields: Vec<(String, MatchTy)>,
}

/// Payload sub-types for each variant of a union, keyed by variant name.
/// A nullary variant has an empty `Vec<MatchTy>`; a tuple-payload variant
/// has one entry per tuple slot. Retained as a convenience type for
/// callers that want to precompute union payloads before constructing
/// the lazy `resolve_variant` closure.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct UnionVariants {
    pub variants: Vec<(String, Vec<MatchTy>)>,
}

/// Build a small `Cx` impl backed by hashmaps the typechecker provides.
pub struct EnvCx<'a> {
    pub enums: &'a HashMap<String, Vec<String>>,
    pub unions: &'a HashMap<String, Vec<String>>,
    pub constructor_arity: &'a HashMap<String, usize>,
    pub records: &'a HashMap<String, RecordFields>,
    /// Closure that resolves a variant's payload sub-types given the
    /// instantiation context. Called lazily during constructor
    /// specialization so nested generic instantiations get the right
    /// substitution at every level. Returns `None` when the variant is
    /// unknown or its payload couldn't be computed; the matrix then
    /// falls back to `Unknown` sub-types.
    pub resolve_variant: &'a (dyn Fn(&str, &MatchTy) -> Option<Vec<MatchTy>> + 'a),
}

impl<'a> Cx for EnvCx<'a> {
    fn ctors_for(&self, ty: &MatchTy) -> CtorSet {
        match ty {
            MatchTy::Bool => CtorSet::Closed(vec![
                CtorInfo { name: "true".to_string(), arity: 0 },
                CtorInfo { name: "false".to_string(), arity: 0 },
            ]),
            MatchTy::Named(name, _) => {
                if let Some(members) = self.enums.get(name) {
                    CtorSet::Closed(
                        members.iter().map(|m| CtorInfo { name: m.clone(), arity: 0 }).collect(),
                    )
                } else if let Some(variants) = self.unions.get(name) {
                    CtorSet::Closed(
                        variants
                            .iter()
                            .map(|v| CtorInfo {
                                name: v.clone(),
                                arity: self.constructor_arity.get(v).copied().unwrap_or(0),
                            })
                            .collect(),
                    )
                } else if self.records.contains_key(name) {
                    // Records are handled as a single struct column in
                    // `is_useful_wild`, but ctors_for needs to return a
                    // non-Unknown set to keep the wildcard probe
                    // recursing correctly. Reporting as `Unlistable`
                    // would force a wildcard arm; instead we mark this
                    // branch dead — the struct logic in is_useful_wild
                    // handles records before we reach ctors_for.
                    CtorSet::Unknown
                } else {
                    CtorSet::Unknown
                }
            }
            MatchTy::Tuple(_) => CtorSet::Unlistable, // tuples are handled via specialize_tuple
            // Lists and records are handled in is_useful_wild directly.
            MatchTy::List(_) => CtorSet::Unknown,
            MatchTy::Record(_) => CtorSet::Unknown,
            // Vector length is effectively unbounded from the type alone,
            // so a wildcard arm is the only way to certify exhaustiveness.
            MatchTy::Vector(_) => CtorSet::Unlistable,
            // Bits(N) is handled in is_useful_wild/collect_witnesses directly.
            MatchTy::Bits(_) => CtorSet::Unlistable,
            MatchTy::Unknown => CtorSet::Unknown,
        }
    }

    fn ctor_sub_tys(&self, name: &str, scrutinee: &MatchTy) -> Vec<MatchTy> {
        let arity = self.constructor_arity.get(name).copied().unwrap_or(0);
        let Some(payload) = (self.resolve_variant)(name, scrutinee) else {
            return std::iter::repeat_n(MatchTy::Unknown, arity).collect();
        };
        // Arity reconciliation: sail unions accept both `Foo(a, b)` and
        // `Foo((a, b))`. If the recorded payload is a single tuple and the
        // matrix arity expects N>1 slots, expand the tuple. If the
        // payload is N>1 flat and the arity expects 1, collapse into a
        // tuple. Mismatches we can't reconcile fall back to Unknowns.
        if payload.len() == arity {
            return payload;
        }
        if arity == 1 && payload.len() > 1 {
            return vec![MatchTy::Tuple(payload)];
        }
        if payload.len() == 1 {
            if let MatchTy::Tuple(items) = &payload[0] {
                if items.len() == arity {
                    return items.clone();
                }
            }
        }
        std::iter::repeat_n(MatchTy::Unknown, arity).collect()
    }

    fn record_field_tys(&self, scrutinee: &MatchTy, fields: &[String]) -> Vec<MatchTy> {
        let record = match scrutinee {
            MatchTy::Record(name) => self.records.get(name),
            _ => None,
        };
        match record {
            Some(info) => fields
                .iter()
                .map(|fname| {
                    info.fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(MatchTy::Unknown)
                })
                .collect(),
            None => std::iter::repeat_n(MatchTy::Unknown, fields.len()).collect(),
        }
    }

    fn record_all_fields(&self, scrutinee: &MatchTy) -> Vec<(String, MatchTy)> {
        match scrutinee {
            MatchTy::Record(name) => {
                self.records.get(name).map(|info| info.fields.clone()).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

/// Maximum bitvector width for exhaustive enumeration. For widths up to
/// this limit we can enumerate all 2^N values and find a concrete missing
/// witness. Beyond this, we report "incomplete" without a specific value.
pub(crate) const BV_WITNESS_MAX_WIDTH: usize = 16;

/// Find a concrete bitvector value not covered by `covered_values`.
///
/// For `width` <= `BV_WITNESS_MAX_WIDTH` (16), enumerates all 2^width
/// values and returns the first uncovered. For larger widths, returns
/// `None` (caller should fall back to a generic wildcard witness).
pub fn find_bitvector_witness(width: usize, covered_values: &[u64]) -> Option<u64> {
    if width == 0 || width > BV_WITNESS_MAX_WIDTH {
        return None;
    }
    let total: u64 = 1u64 << width;
    // Build a set of covered values for O(1) lookup.
    let covered: std::collections::HashSet<u64> = covered_values.iter().copied().collect();
    // If everything is covered, no witness.
    if covered.len() as u64 >= total {
        return None;
    }
    (0..total).find(|v| !covered.contains(v))
}

/// Parse a bitvector literal pattern (binary or hex) into a numeric value.
/// Returns `None` if the literal can't be parsed.
pub fn parse_bv_literal(key: &LiteralKey) -> Option<u64> {
    match key {
        LiteralKey::Binary(s) => {
            let digits = s.trim_start_matches("0b");
            u64::from_str_radix(digits, 2).ok()
        }
        LiteralKey::Hex(s) => {
            let digits = s.trim_start_matches("0x");
            u64::from_str_radix(digits, 16).ok()
        }
        _ => None,
    }
}

/// Format a bitvector value as a binary literal string with the given width.
pub fn format_bv_literal(value: u64, width: usize) -> String {
    format!("0b{:0>width$b}", value, width = width)
}

/// Extract all covered bitvector literal values from the matrix rows at
/// column 0. Only considers `Literal(Binary(_))` and `Literal(Hex(_))`
/// patterns. Returns `None` if any non-literal, non-wildcard pattern is
/// found (meaning we can't enumerate coverage precisely).
pub fn extract_bv_coverage(rows: &[Row]) -> Option<Vec<u64>> {
    let mut values = Vec::new();
    for row in rows {
        if row.pats.is_empty() {
            continue;
        }
        match &row.pats[0] {
            MatchPat::Wild => {
                // A wildcard covers everything — match is exhaustive on
                // this column; caller shouldn't need a witness.
                return None;
            }
            MatchPat::Literal(key) => {
                if let Some(v) = parse_bv_literal(key) {
                    values.push(v);
                }
                // Non-bitvector literals (numbers, strings) are ignored;
                // they don't contribute to bv coverage but also don't
                // invalidate the bv analysis.
            }
            MatchPat::Or(branches) => {
                for branch in branches {
                    if let MatchPat::Literal(key) = branch {
                        if let Some(v) = parse_bv_literal(key) {
                            values.push(v);
                        }
                    }
                }
            }
            // Non-literal, non-wildcard pattern — can't enumerate precisely.
            _ => {}
        }
    }
    Some(values)
}
