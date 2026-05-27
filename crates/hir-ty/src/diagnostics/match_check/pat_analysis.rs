//! Pattern analysis — Maranget matrix algorithm for exhaustiveness and
//! redundancy checking.

use super::{
    extract_bv_coverage, find_bitvector_witness, format_bv_literal, Arm, CtorSet, Cx, LiteralKey,
    MatchPat, MatchTy, Row, UsefulnessReport, BV_WITNESS_MAX_WIDTH,
};

/// Maximum recursion depth before `is_useful`/`is_useful_wild` bail out
/// conservatively. A truly recursive type definition (e.g.
/// `union mu = { Wrap : mu }`) would otherwise drive `is_useful_wild`
/// into unbounded constructor specialization; per-level substitution
/// does not help there. Past the limit we report "not useful" — the
/// conservative outcome (match is covered) so no false-positive
/// incomplete-match diagnostic fires.
const MATCH_CHECK_DEPTH_LIMIT: usize = 64;

/// Complexity limit — maximum number of `is_useful_inner` calls per match.
/// We use a smaller limit to prevent exponential blowup on deeply nested
/// struct types (e.g., list(PMA_Region) where PMA_Region contains PMA
/// with 15 fields including nested enums).
const MATCH_CHECK_COMPLEXITY_LIMIT: usize = 50_000;

// Thread-local step counter for the current match check invocation.
// Reset at the start of `compute_match_usefulness`.
std::thread_local! {
    static MATCH_CHECK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Expand or-patterns inside an arm into multiple rows. The matrix
/// algorithm doesn't see `Or` directly — each branch becomes its own row
/// pointing back at the original arm span.
fn expand_arm_to_rows(arm: &Arm) -> Vec<Row> {
    let mut out = Vec::new();
    expand_one(&arm.pat, arm.arm_span, arm.guard_span.is_some(), &mut out);
    out
}

fn expand_one(pat: &MatchPat, arm_span: crate::Span, has_guard: bool, out: &mut Vec<Row>) {
    match pat {
        MatchPat::Or(branches) => {
            for branch in branches {
                expand_one(branch, arm_span, has_guard, out);
            }
        }
        other => out.push(Row { pats: vec![other.clone()], arm_span, has_guard }),
    }
}

/// The driver. Returns missing witnesses + redundant-arm spans for the
/// given match against `scrutinee_ty`.
pub fn compute_match_usefulness(
    arms: &[Arm],
    scrutinee_ty: &MatchTy,
    cx: &impl Cx,
) -> UsefulnessReport {
    let mut report = UsefulnessReport::default();
    // Reset complexity counter for this match check invocation.
    MATCH_CHECK_STEPS.with(|c| c.set(0));

    // Phase 1 — redundancy. For each arm, check whether its row is
    // useful relative to all earlier (un-guarded) rows. If it's not,
    // the arm is unreachable. We treat guarded arms as never
    // contributing AND never redundant (the guard might subsume them).
    let mut prefix: Vec<Row> = Vec::new();
    for arm in arms {
        let arm_rows = expand_arm_to_rows(arm);
        // Useless arm = no row in `arm_rows` extends the prefix.
        let any_useful = arm_rows
            .iter()
            .any(|row| is_useful(&prefix, row.pats.as_slice(), &[scrutinee_ty.clone()], cx, 0));
        if !any_useful && !arm.guard_span.is_some() {
            report.redundant.push(arm.arm_span);
        }
        // Only un-guarded rows go into the prefix; guarded rows might
        // not actually fire even if they syntactically match.
        for row in arm_rows {
            if !row.has_guard {
                prefix.push(row);
            }
        }
    }

    // Phase 2 — exhaustiveness. Synthesize a "wildcard" probe row of
    // length 1 (matching the single-column scrutinee). If the probe is
    // useful, the match is non-exhaustive — we extract concrete witnesses
    // from the failing constructors.
    let probe = vec![MatchPat::Wild];
    let witnesses = collect_witnesses(&prefix, &probe, &[scrutinee_ty.clone()], cx, 0);
    report.missing_witnesses = witnesses;
    report
}

/// `is_useful(M, p)` — does pattern stack `p` cover any value not
/// already covered by some row of `M`?
fn is_useful(
    matrix: &[Row],
    pats: &[MatchPat],
    tys: &[MatchTy],
    cx: &impl Cx,
    depth: usize,
) -> bool {
    if depth >= MATCH_CHECK_DEPTH_LIMIT {
        return false;
    }
    // Complexity limit — bail out if too many steps.
    let steps = MATCH_CHECK_STEPS.with(|c| {
        let v = c.get() + 1;
        c.set(v);
        v
    });
    if steps > MATCH_CHECK_COMPLEXITY_LIMIT {
        return false;
    }
    if pats.is_empty() {
        // Empty pattern stack: useful iff the matrix has no rows.
        return matrix.is_empty();
    }
    let head = &pats[0];
    let tail = &pats[1..];
    let head_ty = &tys[0];
    let tail_tys = &tys[1..];

    match head {
        MatchPat::Wild | MatchPat::VectorConcat { .. } => {
            is_useful_wild(matrix, tail, head_ty, tail_tys, cx, depth)
        }
        MatchPat::Ctor { name, args } => {
            let mut new_pats = args.clone();
            new_pats.extend_from_slice(tail);
            let sub_tys = cx.ctor_sub_tys(name, head_ty);
            let mut new_tys: Vec<MatchTy> = if sub_tys.len() == args.len() {
                sub_tys
            } else {
                std::iter::repeat(MatchTy::Unknown).take(args.len()).collect()
            };
            new_tys.extend_from_slice(tail_tys);
            let specialized = specialize_ctor(matrix, name, args.len());
            is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1)
        }
        MatchPat::Tuple(items) => {
            let mut new_pats = items.clone();
            new_pats.extend_from_slice(tail);
            // For tuples we ask the cx for sub-types via Tuple shape.
            let sub_tys = match head_ty {
                MatchTy::Tuple(sub) => sub.clone(),
                _ => vec![MatchTy::Unknown; items.len()],
            };
            let mut new_tys = sub_tys;
            new_tys.extend_from_slice(tail_tys);
            let specialized = specialize_tuple(matrix, items.len());
            is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1)
        }
        MatchPat::Literal(key) => {
            let specialized = specialize_literal(matrix, key);
            is_useful(&specialized, tail, tail_tys, cx, depth + 1)
        }
        MatchPat::Nil => {
            let specialized = specialize_nil(matrix);
            is_useful(&specialized, tail, tail_tys, cx, depth + 1)
        }
        MatchPat::Cons(hd, tl) => {
            let elem_ty = match head_ty {
                MatchTy::List(e) => (**e).clone(),
                _ => MatchTy::Unknown,
            };
            let list_ty = head_ty.clone();
            let mut new_pats = vec![(**hd).clone(), (**tl).clone()];
            new_pats.extend_from_slice(tail);
            let mut new_tys = vec![elem_ty, list_ty];
            new_tys.extend_from_slice(tail_tys);
            let specialized = specialize_cons(matrix);
            is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1)
        }
        MatchPat::Vec(items) => {
            let elem_ty = match head_ty {
                MatchTy::Vector(e) => (**e).clone(),
                _ => MatchTy::Unknown,
            };
            let mut new_pats = items.clone();
            new_pats.extend_from_slice(tail);
            let mut new_tys: Vec<MatchTy> = std::iter::repeat(elem_ty).take(items.len()).collect();
            new_tys.extend_from_slice(tail_tys);
            let specialized = specialize_vec(matrix, items.len());
            is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1)
        }
        MatchPat::Struct { fields } => {
            let field_names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            let sub_tys = cx.record_field_tys(head_ty, &field_names);
            let mut new_pats: Vec<MatchPat> = fields.iter().map(|(_, p)| p.clone()).collect();
            new_pats.extend_from_slice(tail);
            let mut new_tys: Vec<MatchTy> = if sub_tys.len() == fields.len() {
                sub_tys
            } else {
                std::iter::repeat(MatchTy::Unknown).take(fields.len()).collect()
            };
            new_tys.extend_from_slice(tail_tys);
            let specialized = specialize_struct(matrix, &field_names);
            is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1)
        }
        MatchPat::Or(_) => {
            // Or-patterns should have been expanded by `expand_arm_to_rows`
            // before we hit the matrix. Defensive fallback: split here.
            let mut found = false;
            if let MatchPat::Or(branches) = head {
                for branch in branches {
                    let mut alt = vec![branch.clone()];
                    alt.extend_from_slice(tail);
                    if is_useful(matrix, &alt, tys, cx, depth + 1) {
                        found = true;
                        break;
                    }
                }
            }
            found
        }
    }
}

/// Wildcard case of `is_useful`. Tries every constructor in the column
/// type. If the constructor set is closed and finite, we recurse into
/// each constructor and report useful iff at least one is uncovered.
fn is_useful_wild(
    matrix: &[Row],
    tail: &[MatchPat],
    head_ty: &MatchTy,
    tail_tys: &[MatchTy],
    cx: &impl Cx,
    depth: usize,
) -> bool {
    if depth >= MATCH_CHECK_DEPTH_LIMIT {
        return false;
    }
    // List types have a closed universe of {Nil, Cons}. Handle them
    // directly so the matrix specialization walks into both branches.
    if let MatchTy::List(elem_ty) = head_ty {
        // Nil branch.
        let nil_specialized = specialize_nil(matrix);
        if is_useful(&nil_specialized, tail, tail_tys, cx, depth + 1) {
            return true;
        }
        // Cons branch.
        let cons_specialized = specialize_cons(matrix);
        let mut cons_pats: Vec<MatchPat> = vec![MatchPat::Wild, MatchPat::Wild];
        cons_pats.extend_from_slice(tail);
        let mut cons_tys: Vec<MatchTy> = vec![(**elem_ty).clone(), head_ty.clone()];
        cons_tys.extend_from_slice(tail_tys);
        if is_useful(&cons_specialized, &cons_pats, &cons_tys, cx, depth + 1) {
            return true;
        }
        return false;
    }
    // Bits(N) for small N: check if all 2^N values are covered by literal
    // patterns. If not all covered, the match is incomplete.
    if let MatchTy::Bits(width) = head_ty {
        if *width <= BV_WITNESS_MAX_WIDTH && *width > 0 {
            // Small bitvector — enumerate coverage.
            if let Some(covered) = extract_bv_coverage(matrix) {
                if find_bitvector_witness(*width, &covered).is_some() {
                    // There's a missing value — the wildcard probe is useful.
                    return true;
                }
                // All values covered — not useful.
                return false;
            }
            // A wildcard exists in the matrix — fall through to default
            // matrix handling.
        }
        // Large bitvector or couldn't extract coverage — fall through to
        // Unlistable handling via ctors_for.
    }
    // Record (struct) types: one virtual constructor with a sub-position
    // per canonical field.
    if let MatchTy::Record(_) = head_ty {
        let field_entries = cx.record_all_fields(head_ty);
        if field_entries.is_empty() {
            // Unknown record — conservative: treat as covered if matrix
            // is non-empty.
            if matrix.is_empty() {
                return true;
            }
            let default = default_matrix(matrix);
            if default.is_empty() {
                return false;
            }
            return is_useful(&default, tail, tail_tys, cx, depth + 1);
        }
        let field_names: Vec<String> = field_entries.iter().map(|(n, _)| n.clone()).collect();
        let specialized = specialize_struct(matrix, &field_names);
        let mut new_pats: Vec<MatchPat> =
            std::iter::repeat(MatchPat::Wild).take(field_names.len()).collect();
        new_pats.extend_from_slice(tail);
        let mut new_tys: Vec<MatchTy> = field_entries.into_iter().map(|(_, t)| t).collect();
        new_tys.extend_from_slice(tail_tys);
        return is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1);
    }
    let ctors = cx.ctors_for(head_ty);
    match ctors {
        CtorSet::Closed(infos) => {
            for info in &infos {
                let sub_tys = cx.ctor_sub_tys(&info.name, head_ty);
                let new_pats = {
                    let mut v: Vec<MatchPat> =
                        std::iter::repeat(MatchPat::Wild).take(info.arity).collect();
                    v.extend_from_slice(tail);
                    v
                };
                let new_tys = {
                    let mut v = sub_tys;
                    v.extend_from_slice(tail_tys);
                    v
                };
                let specialized = specialize_ctor(matrix, &info.name, info.arity);
                if is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1) {
                    return true;
                }
            }
            false
        }
        CtorSet::Unlistable => {
            // Universe of literals (int / string / bits). The only way
            // to certify exhaustiveness is an explicit wildcard arm.
            let default = default_matrix(matrix);
            if matrix.is_empty() {
                return true;
            }
            if default.is_empty() && !matrix.iter().any(|r| matches!(&r.pats[0], MatchPat::Wild)) {
                return true;
            }
            is_useful(&default, tail, tail_tys, cx, depth + 1)
        }
        CtorSet::Unknown => {
            // We don't know the type at this column at all (typically a
            // constructor sub-position whose payload type we didn't
            // track). Be conservative: any non-empty matrix counts as
            // covering the universe — we can't honestly claim a
            // specific witness on a type we couldn't classify.
            if matrix.is_empty() {
                return true;
            }
            // Still recurse into the tail through the default matrix so
            // multi-column matches with a known column further right
            // still get checked. If the default is empty, treat as
            // covered.
            let default = default_matrix(matrix);
            if default.is_empty() {
                return false;
            }
            is_useful(&default, tail, tail_tys, cx, depth + 1)
        }
    }
}

/// Specialize the matrix by constructor `name`/`arity`: keep rows whose
/// head pattern is either that constructor (extracting its sub-patterns)
/// or a wildcard (which becomes `arity` wildcards).
///
/// Sail unions accept both `Foo(tuple)` (1-arg) and `Foo(a, b, c)`
/// (flattened) forms for the same constructor. Rather than tracking
/// multiple arities per constructor, we accept rows whose name matches
/// regardless of args.len() and pad or truncate to the canonical arity
/// with wildcards. This is conservative — it errs on the side of
/// covering — and eliminates the false positives the strict equality
/// check produced on the sail-riscv corpus.
fn specialize_ctor(matrix: &[Row], name: &str, arity: usize) -> Vec<Row> {
    // For booleans the closed ctor set advertises constructors named
    // "true"/"false" with arity 0; rows reach us as `MatchPat::Literal`
    // because the parser lowers `true`/`false` patterns to literals.
    // Treat the two shapes as equivalent so a `Some(true) | Some(false)`
    // match over `option(bool)` is recognized as exhaustive.
    let bool_lit_match = match (name, arity) {
        ("true", 0) => Some(LiteralKey::Bool(true)),
        ("false", 0) => Some(LiteralKey::Bool(false)),
        _ => None,
    };
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                let mut pats: Vec<MatchPat> =
                    std::iter::repeat(MatchPat::Wild).take(arity).collect();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Ctor { name: n, args } if n == name => {
                let mut row_args: Vec<MatchPat> = args.clone();
                row_args.resize(arity, MatchPat::Wild);
                row_args.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats: row_args, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Literal(lit) if bool_lit_match.as_ref() == Some(lit) => {
                out.push(Row {
                    pats: row.pats[1..].to_vec(),
                    arm_span: row.arm_span,
                    has_guard: row.has_guard,
                });
            }
            // Other shapes (Literal, Tuple, mismatched Ctor name) don't
            // specialize — they cover a different part of the universe.
            _ => {}
        }
    }
    out
}

/// Specialize for a Tuple pattern: rows with a tuple head get their
/// elements unpacked into the column; wildcard rows expand to N
/// wildcards. Rows with a non-tuple non-wildcard head are dropped.
fn specialize_tuple(matrix: &[Row], arity: usize) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                let mut pats: Vec<MatchPat> =
                    std::iter::repeat(MatchPat::Wild).take(arity).collect();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Tuple(items) if items.len() == arity => {
                let mut pats = items.clone();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            _ => {}
        }
    }
    out
}

/// Specialize for a literal pattern: keep rows whose head is the same
/// literal or a wildcard.
fn specialize_literal(matrix: &[Row], lit: &LiteralKey) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                out.push(Row {
                    pats: row.pats[1..].to_vec(),
                    arm_span: row.arm_span,
                    has_guard: row.has_guard,
                });
            }
            MatchPat::Literal(other) if other == lit => {
                out.push(Row {
                    pats: row.pats[1..].to_vec(),
                    arm_span: row.arm_span,
                    has_guard: row.has_guard,
                });
            }
            _ => {}
        }
    }
    out
}

/// Specialize for `Nil` (empty list): keep rows whose head is `Nil` or
/// `Wild`, dropping the head column.
fn specialize_nil(matrix: &[Row]) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild | MatchPat::Nil | MatchPat::VectorConcat { .. } => {
                out.push(Row {
                    pats: row.pats[1..].to_vec(),
                    arm_span: row.arm_span,
                    has_guard: row.has_guard,
                });
            }
            _ => {}
        }
    }
    out
}

/// Specialize for `Cons(hd, tl)`: keep rows whose head is `Cons` or
/// `Wild`, expanding the head column into two (head, tail) columns.
fn specialize_cons(matrix: &[Row]) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                let mut pats = vec![MatchPat::Wild, MatchPat::Wild];
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Cons(hd, tl) => {
                let mut pats = vec![(**hd).clone(), (**tl).clone()];
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            _ => {}
        }
    }
    out
}

/// Specialize for a fixed-length `Vec` pattern: rows whose head is a
/// `Vec` of the same length get their elements unpacked; wildcards fan
/// out to `arity` wildcards.
fn specialize_vec(matrix: &[Row], arity: usize) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                let mut pats: Vec<MatchPat> =
                    std::iter::repeat(MatchPat::Wild).take(arity).collect();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Vec(items) if items.len() == arity => {
                let mut pats = items.clone();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            _ => {}
        }
    }
    out
}

/// Specialize for a struct pattern: fan out to one column per field in
/// the given order. Rows whose head is `Wild` expand to N wildcards.
/// Rows whose head is `Struct` have their fields looked up by name (any
/// field not present in the row becomes `Wild`).
fn specialize_struct(matrix: &[Row], field_names: &[String]) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        match &row.pats[0] {
            MatchPat::Wild => {
                let mut pats: Vec<MatchPat> =
                    std::iter::repeat(MatchPat::Wild).take(field_names.len()).collect();
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            MatchPat::Struct { fields } => {
                let mut pats: Vec<MatchPat> = Vec::with_capacity(field_names.len());
                for fname in field_names {
                    let found = fields
                        .iter()
                        .find(|(n, _)| n == fname)
                        .map(|(_, p)| p.clone())
                        .unwrap_or(MatchPat::Wild);
                    pats.push(found);
                }
                pats.extend_from_slice(&row.pats[1..]);
                out.push(Row { pats, arm_span: row.arm_span, has_guard: row.has_guard });
            }
            _ => {}
        }
    }
    out
}

/// "Default" matrix from Maranget: keep only rows whose head is a
/// wildcard, dropping the head column.
fn default_matrix(matrix: &[Row]) -> Vec<Row> {
    matrix
        .iter()
        .filter_map(|row| match &row.pats[0] {
            MatchPat::Wild => Some(Row {
                pats: row.pats[1..].to_vec(),
                arm_span: row.arm_span,
                has_guard: row.has_guard,
            }),
            _ => None,
        })
        .collect()
}


/// Walk the matrix and synthesize concrete patterns that witness each
/// uncovered branch. Mirrors the wildcard probe in `compute_match_usefulness`
/// but recursively builds the missing pattern shape on the way.
fn collect_witnesses(
    matrix: &[Row],
    pats: &[MatchPat],
    tys: &[MatchTy],
    cx: &impl Cx,
    depth: usize,
) -> Vec<MatchPat> {
    if depth >= MATCH_CHECK_DEPTH_LIMIT {
        return Vec::new();
    }
    if pats.is_empty() {
        return if matrix.is_empty() { vec![] } else { vec![] };
    }
    let head = &pats[0];
    let head_ty = &tys[0];
    let tail = &pats[1..];
    let tail_tys = &tys[1..];
    if !matches!(head, MatchPat::Wild) {
        // We only synthesise witnesses from the wildcard probe, so the
        // initial call always sees Wild. Defensive: if not, just return
        // empty.
        return vec![];
    }
    // Bits(N) for small N: produce a concrete missing bitvector literal witness.
    if let MatchTy::Bits(width) = head_ty {
        if *width <= BV_WITNESS_MAX_WIDTH && *width > 0 {
            if let Some(covered) = extract_bv_coverage(matrix) {
                if let Some(missing_val) = find_bitvector_witness(*width, &covered) {
                    let lit_str = format_bv_literal(missing_val, *width);
                    return vec![MatchPat::Literal(LiteralKey::Binary(lit_str))];
                }
                // All covered — no witness.
                return Vec::new();
            }
        }
        // Large bitvector — fall through to Unlistable handling.
    }
    // List: check Nil and Cons branches explicitly, mirroring is_useful_wild.
    if let MatchTy::List(elem_ty) = head_ty {
        let mut witnesses = Vec::new();
        let nil_specialized = specialize_nil(matrix);
        if is_useful(&nil_specialized, tail, tail_tys, cx, depth + 1) {
            witnesses.push(MatchPat::Nil);
        }
        let cons_specialized = specialize_cons(matrix);
        let cons_pats = vec![MatchPat::Wild, MatchPat::Wild];
        let cons_tys = vec![(**elem_ty).clone(), head_ty.clone()];
        let mut full_pats = cons_pats.clone();
        full_pats.extend_from_slice(tail);
        let mut full_tys = cons_tys.clone();
        full_tys.extend_from_slice(tail_tys);
        if is_useful(&cons_specialized, &full_pats, &full_tys, cx, depth + 1) {
            witnesses.push(MatchPat::Cons(Box::new(MatchPat::Wild), Box::new(MatchPat::Wild)));
        }
        return witnesses;
    }
    // Record: single struct "constructor" — if it's uncovered, return
    // a wildcard struct witness.
    if let MatchTy::Record(_) = head_ty {
        let field_entries = cx.record_all_fields(head_ty);
        if field_entries.is_empty() {
            if matrix.is_empty() {
                return vec![MatchPat::Wild];
            }
            return Vec::new();
        }
        let field_names: Vec<String> = field_entries.iter().map(|(n, _)| n.clone()).collect();
        let specialized = specialize_struct(matrix, &field_names);
        let mut new_pats: Vec<MatchPat> =
            std::iter::repeat(MatchPat::Wild).take(field_names.len()).collect();
        new_pats.extend_from_slice(tail);
        let mut new_tys: Vec<MatchTy> = field_entries.iter().map(|(_, t)| t.clone()).collect();
        new_tys.extend_from_slice(tail_tys);
        if is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1) {
            return vec![MatchPat::Struct {
                fields: field_names.into_iter().map(|n| (n, MatchPat::Wild)).collect(),
            }];
        }
        return Vec::new();
    }
    let ctors = cx.ctors_for(head_ty);
    match ctors {
        CtorSet::Closed(infos) => {
            let mut witnesses = Vec::new();
            for info in &infos {
                let sub_tys = cx.ctor_sub_tys(&info.name, head_ty);
                let new_pats = {
                    let mut v: Vec<MatchPat> =
                        std::iter::repeat(MatchPat::Wild).take(info.arity).collect();
                    v.extend_from_slice(tail);
                    v
                };
                let new_tys = {
                    let mut v = sub_tys.clone();
                    v.extend_from_slice(tail_tys);
                    v
                };
                let specialized = specialize_ctor(matrix, &info.name, info.arity);
                if is_useful(&specialized, &new_pats, &new_tys, cx, depth + 1) {
                    // This constructor is uncovered. Build a witness:
                    // `Ctor(_, _, ..., _)` with `arity` wildcards.
                    witnesses.push(MatchPat::Ctor {
                        name: info.name.clone(),
                        args: std::iter::repeat(MatchPat::Wild).take(info.arity).collect(),
                    });
                }
            }
            witnesses
        }
        CtorSet::Unlistable => {
            // Open universe of literals — we can't enumerate concrete
            // witnesses. If there's no wildcard arm, surface a single
            // `_` witness so the diagnostic emission layer can decide
            // whether to suppress it.
            if matrix.is_empty() {
                return vec![MatchPat::Wild];
            }
            if !matrix.iter().any(|r| matches!(&r.pats[0], MatchPat::Wild)) {
                return vec![MatchPat::Wild];
            }
            let default = default_matrix(matrix);
            collect_witnesses(&default, tail, tail_tys, cx, depth + 1)
        }
        CtorSet::Unknown => {
            // Type unknown at this column — refuse to invent a witness.
            // The matching `is_useful_wild` branch already prevents the
            // probe from being marked useful in this case, but we still
            // need a no-op fallback here for safety.
            if matrix.is_empty() {
                vec![MatchPat::Wild]
            } else {
                Vec::new()
            }
        }
    }
}
