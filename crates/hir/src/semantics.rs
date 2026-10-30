//! Primary API to get semantic information, like types, from syntax trees.
//! `Semantics` wraps `SemanticsImpl` and provides typed, position-aware
//! queries.
//!
//! # Architecture
//!
//! - `hir::Semantics` depends ONLY on lower layers (hir-def, hir-ty, base-db)
//! - It does NOT depend on `ide-db` or `ide` -- those depend on `hir`

use base_db::{FileId, FileText};
use hir_def::nameres::DefMap;
use hir_def::resolver::Resolver;
use hir_ty::infer::Ty;

use crate::source_analyzer::SourceAnalyzer;
use std::cell::RefCell;
use std::sync::Arc;

/// Primary API to get semantic information, like types, from syntax trees.
///
/// Uses `&'db dyn salsa::Database` for salsa query access; semantic
/// methods use salsa tracked queries directly.
pub struct Semantics<'db> {
    /// Database reference for salsa queries.
    pub db: &'db dyn salsa::Database,
    imp: SemanticsImpl<'db>,
}

/// Internal implementation.
///
/// Uses salsa queries directly for data fetching. A lightweight
/// most-recent cache avoids re-constructing `SourceAnalyzer` when
/// multiple queries target the same (file, offset) position.
struct SemanticsImpl<'db> {
    db: &'db dyn salsa::Database,
    /// Key: (FileText, offset, infer). Value: the analyzer.
    s2d_cache: RefCell<Option<(FileText, usize, bool, SourceAnalyzer)>>,
}

impl<'db> SemanticsImpl<'db> {
    fn new(db: &'db dyn salsa::Database) -> Self {
        Self { db, s2d_cache: RefCell::new(None) }
    }

    /// Create a `SourceAnalyzer` for a position inside a file.
    /// Uses salsa queries throughout:
    /// - `callable_bodies` for finding the callable at offset
    /// - `file_def_map` for building the resolver
    /// - `infer` for per-callable type inference
    ///
    /// Checks the most-recent cache first to avoid redundant construction
    /// when multiple queries target the same position.
    fn analyze(&self, file_text: FileText, offset: usize, infer: bool) -> Option<SourceAnalyzer> {
        // Check most-recent cache
        {
            let cache = self.s2d_cache.borrow();
            if let Some((cached_ft, cached_off, cached_infer, ref cached_sa)) = *cache {
                // Cache hit if same file+offset, and cached has at least as much
                // info (infer=true covers infer=false)
                if cached_ft == file_text && cached_off == offset && (cached_infer || !infer) {
                    return Some(cached_sa.clone());
                }
            }
        }

        let result = self.analyze_impl(file_text, offset, infer)?;

        // Store in cache
        *self.s2d_cache.borrow_mut() = Some((file_text, offset, infer, result.clone()));

        Some(result)
    }

    /// Core implementation.
    ///
    /// Fully salsa-backed — all data comes from salsa tracked queries.
    fn analyze_impl(
        &self,
        file_text: FileText,
        offset: usize,
        infer: bool,
    ) -> Option<SourceAnalyzer> {
        let db = self.db;
        let file_id = file_text.file_id(db);

        // Use salsa queries for bodies, def_map, and inference.
        let bodies = hir_def::def_query::callable_bodies(db, file_text).as_ref()?;

        // Build DefMap from salsa-cached query — Resolver derived on demand
        let def_map_arc = hir_def::def_query::file_def_map(db, file_text).as_ref()?;
        let resolver = def_map_arc.0.clone();

        // Find the callable containing this offset.
        //
        // There need not be one: a `val`'s own name, a `struct`'s, an
        // `end` marker — every top-level declaration site sits outside
        // every body. This used to give up there, so `resolve_path`
        // answered nothing at exactly those positions and
        // `Definition::classify_at` fell through to `Unresolved` for every
        // top-level name, including names defined in the same file. The
        // file's scope is still an answer, and `new_for_resolver` — which
        // had no caller — is what it is for. rust-analyzer's
        // `Semantics::scope` falls back to the module scope the same way.
        let Some(callable) = bodies.0.entry_at_offset(offset) else {
            return Some(SourceAnalyzer::new_for_resolver(resolver));
        };

        let body = (*callable.body).clone();
        let mut source_map = (*callable.source_map).clone();
        // Ensure file_id is set (does this in callable_bodies,
        // but callable_bodies may not have it)
        if source_map.file_id.is_none() {
            source_map.file_id = Some(file_id);
        }

        if infer {
            // Per-callable inference via salsa
            let inference = self.infer_for_callable(db, file_text, &bodies.0, callable);
            Some(SourceAnalyzer {
                resolver,
                body_or_sig: Some(crate::source_analyzer::BodyOrSig::Body {
                    body,
                    source_map,
                    infer: inference,
                }),
            })
        } else {
            Some(SourceAnalyzer {
                resolver,
                body_or_sig: Some(crate::source_analyzer::BodyOrSig::Body {
                    body,
                    source_map,
                    infer: None,
                }),
            })
        }
    }

    /// Run per-callable inference via salsa tracked query.
    ///
    /// Finds the matching DefWithBodyId and delegates to
    /// `infer`. Salsa memoizes the result so
    /// repeated calls for the same callable are O(1).
    fn infer_for_callable(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        bodies: &hir_def::bodies::CallableBodies,
        target: &hir_def::bodies::CallableBody,
    ) -> Option<hir_ty::InferenceResult> {
        // Compute clause index: count entries with same name before this one
        let target_name = &target.name;
        let mut clause_idx = 0u32;
        for entry in bodies.entries() {
            if std::ptr::eq(entry, target) {
                break;
            }
            if entry.name == *target_name {
                clause_idx += 1;
            }
        }

        // Find matching DefWithBodyId from salsa
        let callable_ids = hir_def::def_query::file_def_with_body_ids(db, file_text);
        let callable_id = callable_ids
            .iter()
            .find(|id| id.name(db) == *target_name && id.clause_index(db) == clause_idx)?;

        // Run inference via salsa tracked query
        let result = hir_ty::query::infer(db, *callable_id);
        Some(result.0.as_ref().clone())
    }
}

impl<'db> Semantics<'db> {
    /// Creates a new `Semantics` instance.
    pub fn new(db: &'db dyn salsa::Database) -> Self {
        let imp = SemanticsImpl::new(db);
        Self { db, imp }
    }

    /// Get the type of an expression at an offset.
    pub fn type_of_expr(&self, file_text: FileText, offset: usize) -> Option<Ty> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.type_of_expr(expr_id).cloned()
    }

    /// Field type of an inferred record owner, including generic arguments.
    pub fn record_field_type(
        &self,
        file: FileText,
        offset: usize,
        owner: &Ty,
        field: &str,
    ) -> Option<Ty> {
        hir_ty::infer::completion::field_type_at(self.db, file, offset, owner, field)
    }

    /// Missing fields of this exact record literal or pattern, with instantiated types.
    /// This editing information is available even when diagnostic policy keeps
    /// an anonymous literal's missing fields quiet.
    pub fn missing_record_fields(
        &self,
        file: FileText,
        range: base_db::TextRange,
    ) -> Option<Vec<(String, String)>> {
        for &id in hir_def::def_query::file_def_with_body_ids(self.db, file) {
            let source_map = hir_def::def_query::body_with_source_map(self.db, id);
            let span = parser::Span::new(range.start().into(), range.end().into());
            if source_map.0 .1.file_id != Some(file.file_id(self.db))
                || (source_map.0 .1.expr_at_span(span).is_none()
                    && source_map.0 .1.pat_at_span(span).is_none())
            {
                continue;
            }
            let inferred = hir_ty::query::infer(self.db, id);
            for (expr, fields) in &inferred.0.missing_record_fields {
                if source_map.0 .1.expr_syntax_in_file(*expr).is_some_and(|node| {
                    node.file_id == file.file_id(self.db) && node.value.text_range() == range
                }) {
                    return Some(fields.clone());
                }
            }
            for diagnostic in &inferred.0.diagnostics {
                let hir_ty::InferenceDiagnostic::MissingPatternFields {
                    pat,
                    missing,
                    field_types,
                    ..
                } = diagnostic
                else {
                    continue;
                };
                if source_map.0 .1.pat_syntax_in_file(*pat).is_some_and(|node| {
                    node.file_id == file.file_id(self.db) && node.value.text_range() == range
                }) {
                    return Some(
                        missing.iter().cloned().zip(field_types.iter().cloned()).collect(),
                    );
                }
            }
        }
        None
    }

    /// Exhaustiveness witnesses for this exact match, from its inferred
    /// scrutinee type and the usefulness analysis of existing guarded arms.
    pub fn missing_match_patterns(
        &self,
        file: FileText,
        range: base_db::TextRange,
    ) -> Option<Vec<String>> {
        for &id in hir_def::def_query::file_def_with_body_ids(self.db, file) {
            let source_map = hir_def::def_query::body_with_source_map(self.db, id);
            let span = parser::Span::new(range.start().into(), range.end().into());
            if source_map.0 .1.file_id != Some(file.file_id(self.db))
                || source_map.0 .1.expr_at_span(span).is_none()
            {
                continue;
            }
            let inferred = hir_ty::query::infer(self.db, id);
            for diagnostic in &inferred.0.diagnostics {
                let hir_ty::InferenceDiagnostic::IncompleteMatch { expr, missing_arms } =
                    diagnostic
                else {
                    continue;
                };
                if source_map.0 .1.expr_syntax_in_file(*expr).is_some_and(|node| {
                    node.file_id == file.file_id(self.db) && node.value.text_range() == range
                }) {
                    return Some(missing_arms.clone());
                }
            }
        }
        None
    }

    /// Get the type of a pattern at an offset.
    pub fn type_of_pat(&self, file_text: FileText, offset: usize) -> Option<Ty> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let pat_id = sa.pat_id(offset)?;
        sa.type_of_pat(pat_id).cloned()
    }

    /// Resolve a name at position to a `PathResolution`.
    /// Takes the salsa handle, as every other method here does — all semantic
    /// queries flow through the database).
    pub fn resolve_path(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let mut external_inline = false;
        if hir_def::source_order::declarations_for(db, file_text).is_some() {
            use hir_def::source_order::{
                self,
                bindings::{self, Namespace},
            };
            if name.starts_with('\'') {
                return Some(crate::PathResolution::TypeVar(crate::TypeVar {
                    name: hir_def::Name::new(name),
                }));
            }
            let at = source_order::at_offset(db, file_text, offset)?;
            let node = source_order::node(db, at)?;
            // A declaration token names itself, even though its initializer
            // or type RHS cannot see that binding yet.
            let at_name = syntax::definition_name_span(&node)
                .is_some_and(|s| s.start <= offset && offset < s.end);
            let outcome_target = at_name && node.kind() == syntax::SyntaxKind::INSTANTIATION_DEF;
            let scattered_type = at_name
                && node.kind() == syntax::SyntaxKind::SCATTERED_CLAUSE_DEF
                && node.children_with_tokens().any(|t| {
                    matches!(t.kind(), syntax::SyntaxKind::KW_ENUM | syntax::SyntaxKind::KW_UNION)
                });
            if at_name && node.kind() == syntax::SyntaxKind::END_DEF {
                let origin = source_order::scattered::end_origin(db, at)?;
                return crate::def_at_declaration(db, origin, name).map(crate::PathResolution::Def);
            }
            if at_name && !outcome_target && !scattered_type {
                if let Some(def) = crate::def_at_declaration(db, at, name) {
                    return Some(crate::PathResolution::Def(def));
                }
            }
            let in_type = scattered_type
                || node
                    .token_at_offset((offset as u32).into())
                    .right_biased()
                    .and_then(|t| t.parent())
                    .is_some_and(|p| p.kind().is_type_node());
            let named_callee = syntax::utils::is_named_callee(&node, offset);
            let namespace = if node.kind() == syntax::SyntaxKind::INSTANTIATION_DEF {
                match syntax::instantiation_reference(&node, offset)? {
                    syntax::InstantiationReference::Outcome => Namespace::Outcome,
                    syntax::InstantiationReference::Type => Namespace::Type,
                    syntax::InstantiationReference::Callable => Namespace::Callable,
                }
            } else if in_type {
                Namespace::Type
            } else if named_callee
                && bindings::exists_at(db, at, hir_def::Name::new(name), Namespace::Callable)
            {
                Namespace::Callable
            } else {
                // Union constructors are represented in the value namespace.
                Namespace::Value
            };
            let called = if named_callee {
                bindings::resolve(db, at, hir_def::Name::new(name), namespace).and_then(|decl| {
                    crate::def_at_declaration_in_namespace(db, decl, name, namespace)
                })
            } else {
                None
            };
            // Locals may share an overload's name. Sharing a concrete
            // function's name is invalid Sail; preserve local error recovery
            // so IDEs do not present that function as the local's signature.
            let overload_callee = matches!(called, Some(crate::Def::Overload(_)));
            if !in_type && !overload_callee {
                if let Some(local) = self
                    .imp
                    .analyze(file_text, offset, false)
                    .and_then(|sa| sa.local_at(db, file_text, offset, name))
                {
                    return Some(crate::PathResolution::Local(local));
                }
            }
            if let Some(def) = called {
                return Some(crate::PathResolution::Def(def));
            }
            if let Some(declaration) =
                bindings::resolve(db, at, hir_def::Name::new(name), namespace)
            {
                return crate::def_at_declaration_in_namespace(db, declaration, name, namespace)
                    .map(crate::PathResolution::Def);
            }
            external_inline =
                matches!(
                    source_order::program(db, file_text),
                    Some(source_order::Program::Inline(_))
                ) && bindings::declarations_named_at(db, at, hir_def::Name::new(name), namespace)
                    .is_empty();
            if !external_inline {
                return None;
            }
        }
        if let Some(resolved) = self
            .imp
            .analyze(file_text, offset, false)
            .and_then(|sa| sa.resolve_path(db, file_text, name))
        {
            return Some(resolved);
        }
        // Project-level resolution shares the same visibility predicate as
        // type inference. Keep returned definitions tied to their source file.
        if !external_inline {
            base_db::module_graph::file_scope(db, file_text.file_id(db))?;
        }
        let ws = base_db::WorkspaceFiles::try_get(db)?;
        for &target in hir_def::visibility::visible_files_for_name(
            db,
            ws,
            file_text.file_id(db),
            hir_def::Name::new(name),
        ) {
            let Some(map) = hir_def::def_query::file_def_map(db, target) else { continue };
            let sa = SourceAnalyzer::new_for_resolver(map.0.clone());
            if let Some(resolved) = sa.resolve_path(db, target, name) {
                return Some(resolved);
            }
        }
        None
    }

    /// Map an `InFile<SyntaxNodePtr>` to a `FileRange` for diagnostic display.
    ///
    /// RA handles macro expansion mapping. Sail has no macros, so this is
    /// an identity mapping: file_id + text_range.
    pub fn diagnostics_display_range(
        &self,
        node: hir_def::in_file::InFile<syntax::SyntaxNodePtr>,
    ) -> hir_def::in_file::FileRange {
        hir_def::in_file::FileRange { file_id: node.file_id, range: node.value.text_range() }
    }

    /// Parse a file through the cached syntax query.
    pub fn parse(&self, file: FileText) -> Option<syntax::SyntaxNode> {
        let parsed = syntax::parse_query::parse_file(self.db, file);
        parsed.green.as_ref().map(|green| syntax::SyntaxNode::new_root((**green).clone()))
    }

    pub fn signature_index(
        &self,
        file: FileText,
    ) -> Arc<std::collections::HashMap<String, hir_def::callable_info::CallableSignature>> {
        crate::semantic_queries::signature_index(self.db, file).clone()
    }

    /// Where a name is mentioned in a file, and which of those mentions
    /// declare it.
    ///
    /// The three places that know what a name is: the item tree for
    /// definitions and the members they declare, the bodies for bindings
    /// and their uses, and the callgraph for call sites.
    ///
    /// `binding_at` names one binding when the caller means a particular
    /// local — two `let x` in a body are two entries in its scope tree, and
    /// a use resolves to whichever the chain reaches first. `top_level`
    /// says the name is declared at file scope, so a local that spells it
    /// the same is a different name.
    ///
    /// This lived in `ide` against a `&dyn FileDb`, which is how the IDE
    /// layer came to hold the semantics rather than ask for them.
    pub fn name_mentions(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        name: &str,
        binding_at: Option<hir_def::Span>,
        top_level: bool,
    ) -> Vec<(hir_def::Span, bool)> {
        let mut spans: Vec<(hir_def::Span, bool)> = Vec::new();
        let key = hir_def::name::Name::new(name);

        // A name the file declares as an enum member or a union constructor
        // is *used* when a pattern names it, not bound: `VI_VADD => 1`
        // matches the member, and lowers to the same `Pat::Bind` a fresh
        // binding would.
        let tree = hir_def::def_query::file_item_tree(db, file_text);
        let tree = tree.as_deref();
        let is_member = tree.is_some_and(|tree| {
            tree.top_level_items().iter().any(|id| {
                matches!(id.def_kind(tree), crate::DefKind::Enum | crate::DefKind::Union)
                    && id.fields(tree).iter().any(|f| f.name.as_str() == name)
            })
        });
        if let Some(tree) = tree {
            for &id in tree.top_level_items() {
                if id.name(tree).as_str() == name {
                    spans.push((
                        id.name_span(tree),
                        id.def_kind(tree) != crate::DefKind::Instantiation,
                    ));
                }
                for field in id.fields(tree) {
                    if field.name.as_str() == name {
                        spans.push((field.name_span, true));
                    }
                }
            }
        }

        if let Some(bodies) = hir_def::def_query::callable_bodies(db, file_text).as_ref() {
            let bodies = &bodies.0;
            // The caller named one binding: resolve every use through the
            // scope tree and keep only those that reach *it*. Two `let x`
            // in one body are two entries in that tree, so a shadowed
            // binding and the one hiding it come apart here.
            if let Some(target) = binding_at {
                for entry in bodies.entries() {
                    let scopes = hir_def::source_order::bindings::body_scopes(
                        db,
                        file_text,
                        &entry.body,
                        entry.body_span.start,
                    );
                    let Some(target_pat) = entry.body.iter_pats().find_map(|(pid, pat)| {
                        (matches!(pat, crate::Pat::Bind(n) if n.as_str() == name)
                            && scopes.binds_pattern(pid)
                            && entry.source_map.pat_syntax(pid) == Some(target))
                        .then_some(pid)
                    }) else {
                        continue;
                    };
                    let mut scoped = vec![(target, true)];
                    for (expr_id, expr) in entry.body.iter_exprs() {
                        let crate::Expr::Ident(n) = expr else { continue };
                        if n.as_str() != name {
                            continue;
                        }
                        let reaches = scopes
                            .scope_for(expr_id)
                            .and_then(|scope| scopes.resolve_name_in_scope(scope, &key))
                            .map(|hit| hit.pat())
                            == Some(target_pat);
                        if reaches {
                            if let Some(span) = entry.source_map.expr_syntax(expr_id) {
                                if span != target {
                                    scoped.push((span, false));
                                }
                            }
                        }
                    }
                    scoped.retain(|(span, _)| span.start < span.end);
                    if scoped.len() > 1 {
                        return scoped;
                    }
                }
            }
            // `binding_at` marks a binding only when some body really binds
            // the name there — a top-level function's resolved symbol
            // carries its own name span too, and treating that as a filter
            // would drop every call site.
            let binds_at = |entry: &crate::CallableBody, target: hir_def::Span| {
                let scopes = hir_def::source_order::bindings::body_scopes(
                    db,
                    file_text,
                    &entry.body,
                    entry.body_span.start,
                );
                entry.body.iter_pats().any(|(pid, pat)| {
                    matches!(pat, crate::Pat::Bind(n) if n.as_str() == name)
                        && scopes.binds_pattern(pid)
                        && entry.source_map.pat_syntax(pid) == Some(target)
                })
            };
            let restrict =
                binding_at.is_some_and(|t| bodies.entries().iter().any(|e| binds_at(e, t)));

            for entry in bodies.entries() {
                if restrict && !binds_at(entry, binding_at.expect("restrict implies a target")) {
                    continue;
                }
                let scopes = hir_def::source_order::bindings::body_scopes(
                    db,
                    file_text,
                    &entry.body,
                    entry.body_span.start,
                );
                for (pat_id, pat) in entry.body.iter_pats() {
                    let Some(span) = entry.source_map.pat_syntax(pat_id) else { continue };
                    match pat {
                        crate::Pat::Bind(n) if n.as_str() == name && is_member => {
                            spans.push((span, false))
                        }
                        crate::Pat::Bind(n) | crate::Pat::TypeVar(n) if n.as_str() == name => {
                            if top_level || !scopes.binds_pattern(pat_id) {
                                continue;
                            }
                            if !restrict || binding_at == Some(span) {
                                spans.push((span, true));
                            }
                        }
                        crate::Pat::App { ctor, .. } if ctor.as_str() == name => {
                            spans.push((span, false))
                        }
                        _ => {}
                    }
                }
                // A use the scope tree resolves to a binding in this body is
                // a use of *that* binding. For a top-level name it is not a
                // reference at all.
                for (expr_id, expr) in entry.body.iter_exprs() {
                    let crate::Expr::Ident(n) = expr else { continue };
                    if n.as_str() != name {
                        continue;
                    }
                    let bound_locally = scopes
                        .scope_for(expr_id)
                        .and_then(|scope| scopes.resolve_name_in_scope(scope, &key))
                        .is_some();
                    if top_level && bound_locally {
                        continue;
                    }
                    if let Some(span) = entry.source_map.expr_syntax(expr_id) {
                        spans.push((span, false));
                    }
                }
            }
        }

        if let Some(graph) = hir_def::def_query::callgraph(db, file_text).as_ref() {
            for site in graph.0.call_sites_in_file() {
                if site.callee == name {
                    spans.push((site.callee_span, false));
                }
            }
        }

        // A name is never empty, so a zero-width span is a missing one.
        spans.retain(|(span, _)| span.start < span.end);
        // One occurrence per position. A call's callee is an `Expr::Ident`
        // that the walk above already reported, and the callgraph reports
        // it again; the two agreed silently while a call site's span was
        // `0..0` and the line above dropped it.
        spans.sort_unstable_by_key(|(span, is_decl)| (span.start, span.end, !*is_decl));
        spans.dedup_by_key(|(span, _)| (span.start, span.end));
        spans
    }

    /// Where each `type_def` in a file declares its name.
    ///
    /// The offset is the *name*'s, which is where a jump should land.
    pub fn type_decls(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
    ) -> Vec<(String, hir_def::Span, crate::DefKind)> {
        let Some(tree) = hir_def::def_query::file_item_tree(db, file_text).as_deref() else {
            return Vec::new();
        };
        tree.top_level_items()
            .iter()
            .filter(|id| {
                matches!(
                    id.def_kind(tree),
                    crate::DefKind::TypeAlias
                        | crate::DefKind::Struct
                        | crate::DefKind::Union
                        | crate::DefKind::Enum
                        | crate::DefKind::Bitfield
                        | crate::DefKind::Newtype
                )
            })
            .map(|id| (id.name(tree).to_string(), id.name_span(tree), id.def_kind(tree)))
            .collect()
    }

    /// `(alias, aliased)` for each `type X = Y` and `newtype X = Y`.
    pub fn type_alias_edges(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
    ) -> Vec<(String, String)> {
        let Some(tree) = hir_def::def_query::file_item_tree(db, file_text).as_deref() else {
            return Vec::new();
        };
        tree.top_level_items()
            .iter()
            .filter(|id| {
                matches!(id.def_kind(tree), crate::DefKind::TypeAlias | crate::DefKind::Newtype)
            })
            .filter_map(|id| {
                let (_, rhs) = id.signature(tree).split_once('=')?;
                let sup = rhs.trim().split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?;
                (!sup.is_empty()).then(|| (id.name(tree).to_string(), sup.to_string()))
            })
            .collect()
    }

    /// Where `name` is *defined* in a file, as the span of each name.
    ///
    /// A `val` or a bare `mapping` signature declares without defining, and
    /// is what `declaration_spans` answers instead. A whole definition wins
    /// over a clause of one, so a jump lands on `function f` rather than on
    /// whichever clause came first.
    pub fn definition_spans(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        name: &str,
    ) -> Vec<hir_def::Span> {
        let Some(tree) = hir_def::def_query::file_item_tree(db, file_text).as_deref() else {
            return Vec::new();
        };
        let defines = |id: &&crate::DefItem| {
            id.name(tree).as_str() == name && !Self::declares_only(id.def_kind(tree))
        };
        let mut spans: Vec<_> = tree
            .top_level_items()
            .iter()
            .filter(defines)
            .filter(|id| !id.is_clause(tree))
            .map(|id| id.name_span(tree))
            .collect();
        if spans.is_empty() {
            spans = tree
                .top_level_items()
                .iter()
                .filter(defines)
                .map(|id| id.name_span(tree))
                .collect();
        }
        spans.sort_unstable_by_key(|span| (span.start, span.end));
        spans.dedup();
        spans
    }

    /// Where `name` is *declared* without being defined.
    pub fn declaration_spans(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        name: &str,
    ) -> Vec<hir_def::Span> {
        let Some(tree) = hir_def::def_query::file_item_tree(db, file_text).as_deref() else {
            return Vec::new();
        };
        tree.top_level_items()
            .iter()
            .filter(|id| id.name(tree).as_str() == name && Self::declares_only(id.def_kind(tree)))
            .map(|id| id.name_span(tree))
            .collect()
    }

    /// A `val`, a bare `mapping` signature, or a scattered head: declares a
    /// name without giving it a body.
    fn declares_only(kind: crate::DefKind) -> bool {
        matches!(
            kind,
            crate::DefKind::ValSpec | crate::DefKind::MappingSpec | crate::DefKind::ScatteredHead
        )
    }

    /// Every mention of the local binding `name` visible at `offset`.
    ///
    /// `None` when no callable containing `offset` binds the name — the
    /// caller should then treat it as a top-level name. `(span, is_binding)`
    /// for each, the binding site first.
    pub fn local_mentions(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        name: &str,
        offset: usize,
    ) -> Option<Vec<(hir_def::Span, bool)>> {
        let bodies = hir_def::def_query::callable_bodies(db, file_text).as_ref()?;
        for entry in bodies.0.entries() {
            // The cursor is inside this callable and this callable binds the
            // name. Asking `expr_at_offset` for an `Ident` instead missed
            // the cursor sitting on a use, because not every expression has
            // a recorded span.
            let inside = offset >= entry.def_span.start
                && offset <= entry.def_span.end
                && entry
                    .body
                    .iter_pats()
                    .any(|(_, pat)| matches!(pat, crate::Pat::Bind(n) if n.as_str() == name));
            if !inside {
                continue;
            }
            let mut spans: Vec<(hir_def::Span, bool)> = Vec::new();
            for (pid, pat) in entry.body.iter_pats() {
                if matches!(pat, crate::Pat::Bind(n) if n.as_str() == name) {
                    if let Some(span) = entry.source_map.pat_syntax(pid) {
                        spans.push((span, true));
                    }
                }
            }
            // A binding with no recorded site is not a local this can speak
            // for; falling through lets the name-based path answer instead
            // of reporting uses with no declaration.
            if spans.is_empty() {
                return None;
            }
            for (eid, expr) in entry.body.iter_exprs() {
                if matches!(expr, crate::Expr::Ident(n) if n.as_str() == name) {
                    if let Some(span) = entry.source_map.expr_syntax(eid) {
                        spans.push((span, false));
                    }
                }
            }
            spans.retain(|(span, _)| span.start < span.end);
            return Some(spans);
        }
        None
    }

    /// Every binding of `name` in a file, wherever it is bound.
    ///
    /// What goto-definition falls back to when nothing at file scope
    /// carries the name: the flat declaration list it replaced held local
    /// declarations too, and matched them without regard to scope.
    pub fn binding_sites(
        &self,
        db: &dyn salsa::Database,
        file_text: FileText,
        name: &str,
    ) -> Vec<hir_def::Span> {
        let Some(bodies) = hir_def::def_query::callable_bodies(db, file_text).as_ref() else {
            return Vec::new();
        };
        bodies
            .0
            .entries()
            .iter()
            .flat_map(|entry| {
                entry.body.iter_pats().filter_map(move |(pid, pat)| {
                    matches!(pat, crate::Pat::Bind(n) if n.as_str() == name)
                        .then(|| entry.source_map.pat_syntax(pid))
                        .flatten()
                })
            })
            .filter(|span| span.start < span.end)
            .collect()
    }

    /// The file that declares the *type* `name`, as seen from `from`.
    ///
    /// The workspace symbol index lives inside a file's environment, and a
    /// feature that has a name and needs its *declaration* — not its type —
    /// had no way to ask. See `hir_ty::query::declaring_file`.
    pub fn declaring_type_file(&self, from: FileText, name: &str) -> Option<FileText> {
        hir_ty::query::declaring_type_file(self.db, from, name)
    }

    /// Resolve a field access expression at the given offset.
    /// Returns the DefIdx of the struct field definition, if the expression
    /// at `offset` is a field access and inference data is available.
    pub fn resolve_field(&self, file_text: FileText, offset: usize) -> Option<hir_def::DefId> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.resolve_field(expr_id)
    }

    /// Resolve a proposed callable name at an arbitrary expression position.
    /// Unlike token navigation, this does not derive the namespace from the
    /// token currently under the cursor. Local shadowing still takes priority.
    pub fn resolve_callable_at(
        &self,
        file: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::Def> {
        use hir_def::source_order::bindings::{resolve, Namespace};
        if self
            .imp
            .analyze(file, offset, false)
            .and_then(|sa| sa.local_at(self.db, file, offset, name))
            .is_some()
        {
            return None;
        }
        let at = hir_def::source_order::at_offset(self.db, file, offset)?;
        if let Some(declaration) =
            resolve(self.db, at, hir_def::Name::new(name), Namespace::Callable)
        {
            return crate::def_at_declaration_in_namespace(
                self.db,
                declaration,
                name,
                Namespace::Callable,
            );
        }
        None
    }

    /// Semantic compatibility for completion ranking. This bounded probe never
    /// runs SMT or commits bindings into a body's inference result.
    pub fn completion_type_match(
        &self,
        file: FileText,
        offset: usize,
        expected: &Ty,
        candidate: &Ty,
    ) -> Option<crate::CompletionTypeMatch> {
        hir_ty::infer::completion::type_match_at(self.db, file, offset, expected, candidate)
    }

    /// Rank a visible callable's declared return types, including overload
    /// members and their quantified predicates, in declaration context.
    pub fn completion_return_type_match(
        &self,
        file: FileText,
        offset: usize,
        expected: &Ty,
        name: &str,
    ) -> Option<crate::CompletionTypeMatch> {
        let offset = if hir_def::source_order::at_offset(self.db, file, offset).is_some() {
            offset
        } else {
            offset.saturating_sub(1)
        };
        if hir_def::source_order::program(self.db, file).is_some() {
            self.resolve_callable_at(file, offset, name)?;
        } else if self
            .imp
            .analyze(file, offset, false)
            .and_then(|sa| sa.local_at(self.db, file, offset, name))
            .is_some()
        {
            return None;
        }
        hir_ty::infer::completion::return_type_match_at(self.db, file, offset, expected, name)
    }

    /// Declared shapes of a proposed callable, respecting source order and
    /// shadowing. Numeric constraints still belong to normal type checking.
    pub fn callable_types_at(&self, file: FileText, offset: usize, name: &str) -> Vec<Ty> {
        self.resolve_callable_at(file, offset, name)
            .map(|def| {
                hir_ty::query::declared_callable_types(self.db, def.location().file_text, name)
            })
            .unwrap_or_default()
    }

    /// Resolve the selected leaf of an overloaded callee at its source point.
    /// Ordinary navigation still resolves the group written by the user.
    pub fn resolve_overload_callee(&self, file: FileText, offset: usize) -> Option<crate::Def> {
        use hir_def::source_order::bindings::{resolve, Namespace};
        let sa = self.imp.analyze(file, offset, true)?;
        let name = sa.infer()?.selected_overloads.get(&sa.expr_id(offset)?)?;
        let at = hir_def::source_order::at_offset(self.db, file, offset)?;
        let selected = resolve(self.db, at, hir_def::Name::new(name), Namespace::Callable)?;
        crate::def_at_declaration_in_namespace(self.db, selected, name, Namespace::Callable)
    }

    /// 投産-4: Resolve a method/function call expression at the given offset.
    /// Returns `(FunctionId, FileId)` identifying the resolved callee, if
    /// the expression at `offset` is a call and inference data is available.
    pub fn resolve_method_call(
        &self,
        file_text: FileText,
        offset: usize,
    ) -> Option<(hir_def::item_id::FunctionId, base_db::FileId)> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.resolve_method_call(expr_id)
    }

    /// Tries field resolution first, then method resolution.
    pub fn resolve_field_or_method(
        &self,
        file_text: FileText,
        offset: usize,
    ) -> Option<either::Either<hir_def::DefId, (hir_def::item_id::FunctionId, base_db::FileId)>>
    {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.resolve_field_or_method(expr_id)
    }

    /// Collect all names visible in the scope at the given offset.
    ///
    /// wraps the same call through a public API on Semantics so that
    /// external crates (ide-completion) can access it without reaching
    /// into `pub(crate)` internals.
    /// The inference result for a whole file, merged across its callables.
    ///
    /// `ide` reaches for the type checker through here rather than calling
    /// `hir_ty::query::*` itself: rust-analyzer's `ide` depends on `hir` and
    /// `syntax` and nothing below them, which is what lets the layers under
    /// `hir` change without touching every feature.
    pub fn infer_file(
        &self,
        file_text: FileText,
    ) -> std::sync::Arc<hir_ty::infer::InferenceResult> {
        hir_ty::query::infer_body(self.db, file_text).0.clone()
    }

    /// The inferred type of an expression inside a named callable.
    ///
    /// The caller has an `ExprId` from a body it already has; what it lacks
    /// is the mapping from a name to the callable that owns it, which is a
    /// question for the semantic layer.
    pub fn expr_ty_in(
        &self,
        file_text: FileText,
        callable: &str,
        expr_id: hir_def::ExprId,
    ) -> Option<hir_ty::Ty> {
        hir_def::def_query::file_def_with_body_ids(self.db, file_text)
            .iter()
            .find(|id| id.name(self.db) == callable)
            .and_then(|&id| hir_ty::query::infer(self.db, id).0.expr_ty(expr_id).cloned())
    }

    /// The `default Order` the workspace declares, if any.
    pub fn workspace_default_order(
        &self,
        files: base_db::WorkspaceFiles,
    ) -> Option<syntax::preprocess::DefaultOrder> {
        hir_def::def_query::workspace_default_order(self.db, files)
    }

    /// A file's item tree.
    pub fn file_item_tree(&self, file_text: FileText) -> Option<std::sync::Arc<hir_def::ItemTree>> {
        hir_def::def_query::file_item_tree(self.db, file_text).clone()
    }

    /// Effects each callable in a file has, including those it inherits from
    /// what it calls.
    pub fn transitive_effects(
        &self,
        file_text: FileText,
    ) -> std::sync::Arc<
        std::collections::HashMap<String, std::collections::BTreeSet<hir_def::EffectTag>>,
    > {
        hir_ty::query::transitive_effects(self.db, file_text).0.clone()
    }

    /// The names a file binds at the top level, by kind.
    pub fn top_level_names(&self, file_text: FileText) -> TopLevelNames {
        let env_data = hir_ty::query::top_level_env(self.db, file_text);
        let env = &env_data.0.env;
        TopLevelNames {
            // A mapping is callable in both directions, and `outcome`
            // declarations register as functions, so both belong in the
            // "bound to something callable" set.
            functions: env.function_names().cloned().chain(env.mapping_names().cloned()).collect(),
            // Scattered `enum clause E = M` members land in `enums`, not
            // `constructors`, but are equally constant patterns.
            constructors: env
                .constructor_names()
                .cloned()
                .chain(env.enum_member_names().cloned())
                .collect(),
            values: env.value_names().cloned().chain(env.register_names().cloned()).collect(),
            records: env.record_names().cloned().collect(),
        }
    }

    /// The inferred type of the expression at `span`, as text.
    pub fn expr_type_text(
        &self,
        file_text: FileText,
        span: parser::Span,
        bodies: Option<&hir_def::bodies::CallableBodies>,
    ) -> Option<String> {
        for &id in hir_def::def_query::file_def_with_body_ids(self.db, file_text) {
            let result = hir_ty::query::infer(self.db, id);
            if let Some(text) = result.0.expr_type_text(span, bodies) {
                return Some(text);
            }
        }
        None
    }

    /// The inferred type of the binding at `span`, as text.
    pub fn binding_type_text(
        &self,
        file_text: FileText,
        span: parser::Span,
        bodies: Option<&hir_def::bodies::CallableBodies>,
    ) -> Option<String> {
        for &id in hir_def::def_query::file_def_with_body_ids(self.db, file_text) {
            let result = hir_ty::query::infer(self.db, id);
            if let Some(text) = result.0.binding_type_text(span, bodies) {
                return Some(text);
            }
        }
        None
    }

    pub fn names_in_scope(&self, file_text: FileText, offset: usize) -> Vec<hir_def::Name> {
        let sa = match self.imp.analyze(file_text, offset, false) {
            Some(sa) => sa,
            None => return Vec::new(),
        };
        let resolver = sa.make_resolver();
        // The file's names, then the ones the *body* binds at this offset.
        // The resolver a `SemanticsScope` builds is `Resolver::for_file`,
        // which carries no expression scope, so this answered with the
        // top-level names alone.
        let mut names = if hir_def::source_order::declarations_for(self.db, file_text).is_some() {
            hir_def::source_order::at_offset(self.db, file_text, offset)
                .map(|at| {
                    hir_def::source_order::bindings::names_at(self.db, at.file, at.index).clone()
                })
                .unwrap_or_default()
        } else {
            resolver.names_in_scope()
        };
        names.extend(sa.local_names_at(self.db, file_text, offset));
        names.sort();
        names.dedup();
        names
    }

    /// The locals in scope at `offset`, innermost first, with the span each
    /// is bound at — for a name that is not written there yet, which
    /// `resolve_path` cannot resolve because it resolves a mention.
    pub fn locals_in_scope(&self, file_text: FileText, offset: usize) -> Vec<crate::Local> {
        self.imp
            .analyze(file_text, offset, false)
            .map(|sa| sa.locals_at(self.db, file_text, offset))
            .unwrap_or_default()
    }

    pub fn local_names_in_scope(&self, file_text: FileText, offset: usize) -> Vec<hir_def::Name> {
        self.imp
            .analyze(file_text, offset, false)
            .map(|sa| sa.local_names_at(self.db, file_text, offset))
            .unwrap_or_default()
    }
}

/// A scope at a specific position in a file.
///
/// Stores `Arc<DefMap>` and derives `Resolver<'_>` on demand.
/// Callers pass `db` to `resolve_name` explicitly.
#[derive(Debug)]
pub struct SemanticsScope {
    file_id: FileId,
    resolver: Arc<DefMap>,
}

impl SemanticsScope {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn make_resolver(&self) -> Resolver<'_> {
        Resolver::for_file(&self.resolver)
    }
}

/// What can appear in a scope.
///
/// Simplified for Sail (no const generics, no loop labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeDef {
    /// A top-level definition (function, type, register, etc.).
    Def(hir_def::Name),
    /// A local variable binding.
    Local(crate::Local),
    /// A type variable.
    TypeVar(crate::TypeVar),
    /// Unknown / unresolved.
    Unknown,
}

impl SemanticsScope {
    /// Resolve a name in this scope.
    ///
    /// Takes `db``).
    pub fn resolve_name(
        &self,
        db: &dyn salsa::Database,
        file_text: base_db::FileText,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let resolver = self.make_resolver();
        if let Some(type_ns) = resolver.resolve_path_in_type_ns(db, name) {
            return Some(crate::PathResolution::from_type_ns(db, file_text, type_ns));
        }
        if let Some(value_ns) = resolver.resolve_path_in_value_ns(db, name) {
            return Some(crate::PathResolution::from_value_ns(db, file_text, value_ns));
        }
        None
    }
}

/// The names a file binds at the top level, grouped by what they name.
///
/// `ide` used to reach into `hir_ty`'s `TopLevelEnv` for these; the shape it
/// actually wanted is this much smaller one.
#[derive(Debug, Default, Clone)]
pub struct TopLevelNames {
    pub functions: Vec<String>,
    pub constructors: Vec<String>,
    /// Top-level `let`/`var` bindings and registers alike — both are names
    /// bound to a value.
    pub values: Vec<String>,
    pub records: Vec<String>,
}
