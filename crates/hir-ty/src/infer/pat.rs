//! Pattern type inference.
//!
//! Adds `impl InferenceContext` methods for pattern inference, called
//! from expression inference (match arms, let bindings, foreach, etc.).
//!
//! # Key methods
//!
//! - `bind_pattern_hir`: Main entry — recursively binds pattern variables
//!   with their inferred types based on the expected (scrutinee) type.
//! - `collect_hir_pat_bindings`: Collects all binding names in a pattern tree.
//! - `collect_vector_subrange_parts_hir`: Extracts vector subrange info.

use std::collections::{HashMap, HashSet};

use parser::Span;

use hir_def::Pat;

use super::constraint;
use super::expr::InferenceContext;
use super::is_pattern_binding;
use super::{Subst, Ty, TyArg, TyKind};
use crate::infer::{InferenceDiagnostic, LocalEnv};

impl<'db> InferenceContext<'db> {
    /// Infer types for a pattern, binding variables in `locals`.
    /// Recursively walks the pattern tree, unifying each sub-pattern
    /// with the expected type and recording bindings in the local
    /// environment.
    pub(super) fn bind_pattern_hir(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        expected: &Ty,
        locals: &mut LocalEnv,
    ) {
        // Fuel guard
        self.inference_fuel = self.inference_fuel.saturating_sub(1);
        if self.inference_fuel == 0 {
            return;
        }
        // Write pattern type to InferenceResult.
        self.result.write_pat_ty(pat_id, expected.clone());
        let Some(pat) = body.pat(pat_id) else { return };
        match pat {
            Pat::Wild | Pat::Missing => {}
            Pat::Literal(lit) => {
                let lit_ty = super::infer_literal_type(lit);
                if !expected.is_error()
                    && !lit_ty.is_error()
                    && !self.table.unify(expected, &lit_ty)
                {
                    self.result.record_type_mismatch_at(
                        hir_def::ExprOrPatId::PatId(pat_id),
                        expected,
                        &lit_ty,
                    );
                }
            }
            Pat::Bind(name) => {
                // Duplicate detection is done per-pattern-tree (not against
                // enclosing scope) in check_pattern_duplicates.
                locals.define(name, expected.clone());
                if let Some(span) = self.pat_span(body, pat_id) {
                    self.record_binding_type(span, expected);
                }
            }
            Pat::TypeVar(name) => {
                locals.define(name, expected.clone());
                let value_name = name.strip_prefix('\'').unwrap_or(name);
                if value_name != name {
                    locals.define(value_name, expected.clone());
                }
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                self.bind_pattern_hir(body, *inner, expected, locals);
            }
            Pat::As { pat, binding } => {
                self.bind_pattern_hir(body, *pat, expected, locals);
                locals.define(binding, expected.clone());
            }
            Pat::Tuple(items) => {
                if let TyKind::Tuple(item_tys) = expected.kind() {
                    for (pat_id, ty) in items.iter().zip(item_tys.iter()) {
                        self.bind_pattern_hir(body, *pat_id, ty, locals);
                    }
                } else {
                    for &pid in items {
                        self.bind_pattern_hir(body, pid, &Ty::error(), locals);
                    }
                }
            }
            Pat::App { ctor, args } => {
                let candidates = self.env.lookup_functions(ctor);
                if let Some(scheme) = candidates.first() {
                    let _ = self.table.unify(&scheme.ret, expected);
                    let mut subst = Subst::default();
                    self.extract_value_bindings(&scheme.ret, expected, &mut subst);
                    for (pid, param_ty) in args.iter().zip(scheme.params.iter()) {
                        let ty = constraint::apply_subst(param_ty, &subst);
                        self.bind_pattern_hir(body, *pid, &ty, locals);
                    }
                    // Record variant/constructor resolution for this pattern.
                    // Enables goto-def for constructor patterns (e.g., `Some(x)`).
                    let ctor_name = hir_def::Name::from(ctor.as_str());
                    let per_ns = self.make_resolver().def_map().root_scope().get(&ctor_name);
                    if let Some(ctor_item) = per_ns.values.or(per_ns.types) {
                        self.result.record_variant_resolution(
                            hir_def::ExprOrPatId::PatId(pat_id),
                            ctor_item.def,
                        );
                    }
                } else {
                    let mapping_schemes = self.env.lookup_mappings(ctor);
                    if let Some(ms) = mapping_schemes.first() {
                        let payload_ty = if !expected.is_error() {
                            if self.table.try_unify(&ms.lhs, expected) {
                                let mut subst = Subst::default();
                                self.extract_value_bindings(&ms.lhs, expected, &mut subst);
                                constraint::apply_subst(&ms.rhs, &subst)
                            } else if self.table.try_unify(&ms.rhs, expected) {
                                let mut subst = Subst::default();
                                self.extract_value_bindings(&ms.rhs, expected, &mut subst);
                                constraint::apply_subst(&ms.lhs, &subst)
                            } else {
                                ms.rhs.clone()
                            }
                        } else {
                            ms.rhs.clone()
                        };
                        for &pid in args {
                            self.bind_pattern_hir(body, pid, &payload_ty, locals);
                        }
                    } else {
                        for &pid in args {
                            self.bind_pattern_hir(body, pid, &Ty::error(), locals);
                        }
                    }
                }
            }
            Pat::List(items) | Pat::Array(items) => {
                for &pid in items {
                    self.bind_pattern_hir(body, pid, &Ty::error(), locals);
                }
            }
            Pat::Struct { name, fields } => {
                let record_name = name.as_ref().or_else(|| match expected.kind() {
                    TyKind::Adt(n, _) => Some(n),
                    TyKind::App { name: n, .. } => Some(n),
                    _ => None,
                });
                let record_info = record_name
                    .and_then(|rn| self.env.records.get(rn.as_str()).cloned())
                    .map(|r| (record_name.unwrap().clone(), r));
                if let Some((_rname, record)) = &record_info {
                    for (fname, pid) in fields {
                        let field_ty =
                            record.fields.get(fname.as_str()).cloned().unwrap_or(Ty::error());
                        self.bind_pattern_hir(body, *pid, &field_ty, locals);
                    }
                    let provided: HashSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                    // A `_` field name is a wildcard meaning "ignore remaining
                    // fields", so skip the missing-fields diagnostic.
                    let has_wildcard = provided.contains("_");
                    let mut missing: Vec<&str> = record
                        .fields
                        .keys()
                        .filter(|k| !provided.contains(k.as_str()))
                        .map(|s| s.as_str())
                        .collect();
                    if !missing.is_empty() && !has_wildcard {
                        missing.sort();
                        let rname = record_name.cloned().unwrap_or_default();
                        self.push_inference_diagnostic(InferenceDiagnostic::MissingPatternFields {
                            pat: pat_id,
                            record_name: rname,
                            missing: missing.iter().map(|s| s.to_string()).collect(),
                        });
                    }
                } else {
                    for (_, pid) in fields {
                        self.bind_pattern_hir(body, *pid, &Ty::error(), locals);
                    }
                }
            }
            Pat::Infix { lhs, op, rhs } => {
                if op == "@" {
                    let mut subranges = Vec::new();
                    self.collect_vector_subrange_parts_hir(body, pat_id, &mut subranges);
                    if !subranges.is_empty() {
                        let name = &subranges[0].0;
                        let total_width: usize = subranges.iter().map(|(_, w, _, _)| w).sum();
                        let bind_ty = if total_width > 0 {
                            Ty::app(
                                "bits",
                                vec![TyArg::numeric(total_width.to_string())],
                                format!("bits({total_width})"),
                            )
                        } else {
                            expected.clone()
                        };
                        locals.define(name, bind_ty);
                        if subranges.len() >= 2 {
                            let mut ranges: Vec<(usize, usize)> = Vec::new();
                            for (_, _, hi, lo) in &subranges {
                                if let (Some(h), Some(l)) = (hi, lo) {
                                    ranges.push((*h, *l));
                                }
                            }
                            ranges.sort_by_key(|b| std::cmp::Reverse(b.0));
                            for i in 1..ranges.len() {
                                let expected_hi = ranges[i - 1].1.wrapping_sub(1);
                                if ranges[i].0 != expected_hi {
                                    self.push_inference_diagnostic(
                                        InferenceDiagnostic::NonContiguousSubrange { pat: pat_id },
                                    );
                                    break;
                                }
                            }
                        }
                    } else {
                        self.bind_pattern_hir(body, *lhs, &Ty::error(), locals);
                        self.bind_pattern_hir(body, *rhs, &Ty::error(), locals);
                    }
                } else {
                    self.bind_pattern_hir(body, *lhs, &Ty::error(), locals);
                    self.bind_pattern_hir(body, *rhs, &Ty::error(), locals);
                }
            }
            Pat::Index { name, .. } => {
                if is_pattern_binding(name, &self.pattern_constants, self.env.has_workspace_context)
                {
                    locals.define(name, Ty::named("bit".to_string()));
                }
            }
            Pat::RangeIndex { name, start_span, end_span } => {
                if is_pattern_binding(name, &self.pattern_constants, self.env.has_workspace_context)
                {
                    let start_text = &self.source[start_span.start..start_span.end];
                    let end_text = &self.source[end_span.start..end_span.end];
                    let width = start_text.parse::<usize>().ok().and_then(|s| {
                        end_text.parse::<usize>().ok().map(|e| {
                            if s >= e {
                                s - e + 1
                            } else {
                                e - s + 1
                            }
                        })
                    });
                    if let Some(w) = width {
                        locals.define(
                            name,
                            Ty::app(
                                "bits",
                                vec![TyArg::numeric(w.to_string())],
                                format!("bits({w})"),
                            ),
                        );
                    } else {
                        locals.define(name, expected.clone());
                    }
                }
            }
        }
    }

    /// Collect binding names and spans from a Pat tree.
    pub(super) fn collect_hir_pat_bindings(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
    ) -> HashMap<String, Span> {
        let mut out = HashMap::new();
        self.collect_hir_pat_bindings_inner(body, pat_id, &mut out);
        out
    }

    pub(super) fn collect_hir_pat_bindings_inner(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        out: &mut HashMap<String, Span>,
    ) {
        let Some(pat) = body.pat(pat_id) else { return };
        let span = self.pat_span(body, pat_id).unwrap_or(Span::new(0, 0));
        match pat {
            Pat::Bind(name)
                if is_pattern_binding(
                    name,
                    &self.pattern_constants,
                    self.env.has_workspace_context,
                ) =>
            {
                out.entry(name.clone()).or_insert(span);
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                self.collect_hir_pat_bindings_inner(body, *inner, out);
            }
            Pat::As { pat, binding } => {
                self.collect_hir_pat_bindings_inner(body, *pat, out);
                out.entry(binding.clone()).or_insert(span);
            }
            Pat::Tuple(items) | Pat::List(items) | Pat::Array(items) => {
                for &pid in items {
                    self.collect_hir_pat_bindings_inner(body, pid, out);
                }
            }
            Pat::App { args, .. } => {
                for &pid in args {
                    self.collect_hir_pat_bindings_inner(body, pid, out);
                }
            }
            Pat::Struct { fields, .. } => {
                for (_, pid) in fields {
                    self.collect_hir_pat_bindings_inner(body, *pid, out);
                }
            }
            Pat::Infix { lhs, rhs, .. } => {
                self.collect_hir_pat_bindings_inner(body, *lhs, out);
                self.collect_hir_pat_bindings_inner(body, *rhs, out);
            }
            _ => {}
        }
    }

    /// Collect vector subrange pattern parts from a `@` infix pattern tree.
    /// Returns (name, width, high_index, low_index) for each part.
    pub(super) fn collect_vector_subrange_parts_hir(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        out: &mut Vec<(String, usize, Option<usize>, Option<usize>)>,
    ) {
        let Some(pat) = body.pat(pat_id) else { return };
        match pat {
            Pat::Infix { lhs, op, rhs } if op == "@" => {
                self.collect_vector_subrange_parts_hir(body, *lhs, out);
                self.collect_vector_subrange_parts_hir(body, *rhs, out);
            }
            Pat::RangeIndex { name, start_span, end_span } => {
                let start_text = &self.source[start_span.start..start_span.end];
                let end_text = &self.source[end_span.start..end_span.end];
                let hi = start_text.parse::<usize>().ok();
                let lo = end_text.parse::<usize>().ok();
                let width = hi
                    .and_then(|h| lo.map(|l| if h >= l { h - l + 1 } else { l - h + 1 }))
                    .unwrap_or(0);
                out.push((name.clone(), width, hi, lo));
            }
            Pat::Index { name, .. } => {
                out.push((name.clone(), 1, None, None));
            }
            _ => {}
        }
    }

    /// Check for duplicate bindings within a single pattern tree.
    ///
    /// Collects all `Pat::Bind` names, then reports duplicates.
    /// Excludes enum members/constructors.
    pub(super) fn check_pattern_duplicates(
        &mut self,
        body: &hir_def::Body,
        root_pat: hir_def::PatId,
    ) {
        let mut seen: HashMap<String, hir_def::PatId> = HashMap::new();
        self.collect_pattern_names_for_dup_check(body, root_pat, &mut seen);
    }

    /// Check if `name` appears as a binding in ANY pattern in the body.
    /// Fallback for complex patterns (mapping arms, subrange patterns)
    /// that bind_pattern_hir didn't register in locals.
    pub(super) fn name_appears_in_body_patterns(&self, body: &hir_def::Body, name: &str) -> bool {
        for &pat_id in body.params.iter() {
            if self.pat_tree_contains_name(body, pat_id, name) {
                return true;
            }
        }
        for arm in &body.mapping_arms {
            if let Some(pat_id) = arm.lhs_pat {
                if self.pat_tree_contains_name(body, pat_id, name) {
                    return true;
                }
            }
            if let Some(pat_id) = arm.rhs_pat {
                if self.pat_tree_contains_name(body, pat_id, name) {
                    return true;
                }
            }
        }
        false
    }

    /// Heuristic: check if `name` likely appears as a pattern binding
    /// elsewhere in the source file (scattered mapping clauses, etc.).
    /// Looks for `name[`, `name @`, `name,`, `name)` patterns in source.
    #[allow(dead_code)] // Kept for potential future use in filtering
    pub(super) fn name_likely_pattern_binding(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        // Check all occurrences of `name` in source text
        let src = self.source;
        let mut pos = 0;
        while let Some(idx) = src[pos..].find(name) {
            let abs = pos + idx;
            let end = abs + name.len();
            // Verify word boundary (not substring of larger ident)
            let pre_ok = abs == 0 || !src.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let post_ok = end >= src.len() || !src.as_bytes()[end].is_ascii_alphanumeric();
            if pre_ok && post_ok && end < src.len() {
                let next = src.as_bytes()[end];
                // Pattern indicators: name[ name@ name, name) name:
                if matches!(next, b'[' | b'@' | b',' | b')' | b':') {
                    return true;
                }
                // Check with whitespace: `name :` `name (`
                if next == b' ' && end + 1 < src.len() {
                    let next2 = src.as_bytes()[end + 1];
                    if matches!(next2, b':' | b'(' | b'@') {
                        return true;
                    }
                }
                // Preceded by `(` or `,` (parameter position)
                if abs > 0 {
                    let prev = src.as_bytes()[abs - 1];
                    if matches!(prev, b'(' | b',') && matches!(next, b' ' | b'\n' | b'\r') {
                        return true;
                    }
                }
            }
            pos = abs + 1;
            if pos >= src.len() {
                break;
            }
        }
        false
    }

    fn pat_tree_contains_name(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        name: &str,
    ) -> bool {
        let Some(pat) = body.pat(pat_id) else { return false };
        match pat {
            Pat::Bind(n) | Pat::TypeVar(n) => n == name,
            Pat::As { pat, binding } => {
                binding == name || self.pat_tree_contains_name(body, *pat, name)
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                self.pat_tree_contains_name(body, *inner, name)
            }
            Pat::App { args, .. } => {
                args.iter().any(|a| self.pat_tree_contains_name(body, *a, name))
            }
            Pat::Tuple(items) | Pat::Array(items) | Pat::List(items) => {
                items.iter().any(|a| self.pat_tree_contains_name(body, *a, name))
            }
            Pat::Infix { lhs, rhs, .. } => {
                self.pat_tree_contains_name(body, *lhs, name)
                    || self.pat_tree_contains_name(body, *rhs, name)
            }
            Pat::Index { name: n, .. } | Pat::RangeIndex { name: n, .. } => n == name,
            Pat::Struct { fields, .. } => {
                fields.iter().any(|(_, p)| self.pat_tree_contains_name(body, *p, name))
            }
            _ => false,
        }
    }

    fn collect_pattern_names_for_dup_check(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        seen: &mut HashMap<String, hir_def::PatId>,
    ) {
        let Some(pat) = body.pat(pat_id) else { return };
        match pat.clone() {
            Pat::Bind(name) => {
                // Exclude enum members / constructors
                if !is_pattern_binding(
                    &name,
                    &self.pattern_constants,
                    self.env.has_workspace_context,
                ) {
                    return;
                }
                match seen.entry(name) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                            pat: pat_id,
                            name: e.key().clone(),
                        });
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(pat_id);
                    }
                }
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                self.collect_pattern_names_for_dup_check(body, inner, seen);
            }
            Pat::As { pat, binding } => {
                self.collect_pattern_names_for_dup_check(body, pat, seen);
                if is_pattern_binding(
                    &binding,
                    &self.pattern_constants,
                    self.env.has_workspace_context,
                ) {
                    match seen.entry(binding) {
                        std::collections::hash_map::Entry::Occupied(e) => {
                            self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                                pat: pat_id,
                                name: e.key().clone(),
                            });
                        }
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(pat_id);
                        }
                    }
                }
            }
            Pat::Tuple(items) | Pat::List(items) | Pat::Array(items) => {
                for pid in items {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::App { args, .. } => {
                for pid in args {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::Struct { fields, .. } => {
                // Check for duplicate field names
                let mut seen_fields: HashMap<String, hir_def::PatId> = HashMap::new();
                for (fname, pid) in &fields {
                    if let Some(_prev) = seen_fields.get(fname) {
                        self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                            pat: *pid,
                            name: fname.clone(),
                        });
                    } else {
                        seen_fields.insert(fname.clone(), *pid);
                    }
                }
                for (_, pid) in fields {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::Infix { lhs, rhs, .. } => {
                self.collect_pattern_names_for_dup_check(body, lhs, seen);
                self.collect_pattern_names_for_dup_check(body, rhs, seen);
            }
            _ => {} // Wild, Missing, Literal, Index — no bindings
        }
    }
}
