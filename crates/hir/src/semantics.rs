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

/// A source-written parameter in a visible callable clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableParameter {
    pub name: String,
    pub source: Option<crate::FileRange>,
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

        // Use salsa queries for bodies, def_map, and inference.
        // Build DefMap from salsa-cached query — Resolver derived on demand
        let def_map_arc = hir_def::def_query::file_def_map(db, file_text).as_ref()?;
        let resolver = def_map_arc.0.clone();
        let Some(bodies) = hir_def::def_query::callable_bodies_at_offset(db, file_text, offset)
        else {
            return Some(SourceAnalyzer::new_for_resolver(resolver));
        };

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

        let body = callable.body.clone();
        let source_map = callable.source_map.clone();

        let inference = if infer {
            Some(self.infer_for_callable(db, file_text, &bodies.0, callable))
        } else {
            None
        };
        Some(SourceAnalyzer {
            resolver,
            body: Some(crate::source_analyzer::BodyAnalysis { body, source_map, infer: inference }),
        })
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
    ) -> Arc<hir_ty::InferenceResult> {
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
        let callable_id =
            hir_def::def_query::DefWithBodyId::new(db, file_text, target_name.clone(), clause_idx);

        // Run inference via salsa tracked query
        let result = hir_ty::query::infer(db, callable_id);
        result.0.clone()
    }
}

impl<'db> Semantics<'db> {
    /// Creates a new `Semantics` instance.
    pub fn new(db: &'db dyn salsa::Database) -> Self {
        let imp = SemanticsImpl::new(db);
        Self { db, imp }
    }

    /// Declaration constraints use the same lexical proof environment as bodies.
    /// Repeated diagnostics reuse the tracked result, including imported facts.
    pub fn declaration_diagnostics(
        &self,
        file: FileText,
    ) -> &'db Vec<crate::diagnostics::Diagnostic> {
        hir_ty::infer::declaration_constraints::declaration_diagnostics(self.db, file)
    }

    pub fn bodies_at_offset(
        &self,
        file: FileText,
        offset: usize,
    ) -> Option<&'db hir_def::CallableBodies> {
        hir_def::def_query::callable_bodies_at_offset(self.db, file, offset).map(|group| &*group.0)
    }

    pub fn bodies_in_range(
        &self,
        file: FileText,
        range: Option<(usize, usize)>,
    ) -> Vec<&'db hir_def::CallableBodies> {
        hir_def::def_query::callable_bodies_in_range(self.db, file, range)
            .into_iter()
            .map(|group| &*group.0)
            .collect()
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
                    binding: None,
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
            if namespace == Namespace::Type {
                // Type resolution includes compiler builtins after checking
                // source declarations. The generic ordered value resolver
                // must not turn a primitive annotation into an unknown name.
                return self.resolve_type_path(file_text, offset, name);
            }
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

    /// Resolve a type name in an explicitly selected namespace, independent
    /// of the syntax at the invocation cursor (used by type-pattern SSR).
    pub fn resolve_type_path(
        &self,
        file: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::PathResolution> {
        use hir_def::source_order::{
            self,
            bindings::{self, Namespace},
        };
        if let Some(at) = source_order::at_offset(self.db, file, offset) {
            if let Some(declaration) =
                bindings::resolve(self.db, at, hir_def::Name::new(name), Namespace::Type)
            {
                return crate::def_at_declaration_in_namespace(
                    self.db,
                    declaration,
                    name,
                    Namespace::Type,
                )
                .map(crate::PathResolution::Def);
            }
            return (name.starts_with('\'') || hir_def::is_builtin_type_name(name)).then(|| {
                crate::PathResolution::TypeVar(crate::TypeVar {
                    name: hir_def::Name::new(name),
                    binding: None,
                })
            });
        }
        let analyzer = self.imp.analyze(file, offset, false)?;
        let resolver = analyzer.make_resolver();
        resolver
            .resolve_path_in_type_ns(self.db, name)
            .map(|ns| crate::PathResolution::from_type_ns(self.db, file, ns))
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

    /// Body scopes and cross-file visibility have one semantic entry point.
    pub fn body_scopes(
        &self,
        file: FileText,
        callable: &hir_def::bodies::CallableBody,
    ) -> crate::ExprScopes {
        hir_def::source_order::bindings::body_scopes(
            self.db,
            file,
            &callable.body,
            callable.body_span.start,
        )
    }

    pub fn symbol_visible_from(&self, from: FileId, target: FileText, name: &str) -> bool {
        hir_def::visibility::symbol_visible_from(self.db, from, target, name)
    }

    /// Source parameters selected through canonical declaration identities.
    pub fn callable_parameters_at(
        &self,
        file: base_db::FileText,
        offset: usize,
        name: &str,
    ) -> Option<Vec<Vec<CallableParameter>>> {
        let db = self.db;
        use hir_def::source_order::{
            self,
            bindings::{self, Namespace},
        };
        source_order::declarations_for(db, file)?;
        if matches!(
            self.resolve_path(db, file, offset, name),
            Some(crate::PathResolution::Local(_))
        ) {
            return Some(Vec::new());
        }
        let at = source_order::at_offset(db, file, offset)?;
        if matches!(source_order::program(db, file), Some(source_order::Program::Inline(_)))
            && !bindings::contains_name(db, at, &hir_def::Name::new(name))
        {
            return None;
        }
        let members = bindings::overload_leaf_members(db, at, hir_def::Name::new(name))
            .unwrap_or_else(|| vec![hir_def::Name::new(name)]);
        let mut result = Vec::new();
        for member in members {
            db.unwind_if_revision_cancelled();
            let Some(selected) = bindings::resolve(db, at, member.clone(), Namespace::Callable)
                .and_then(|decl| {
                    crate::def_at_declaration_in_namespace(
                        db,
                        decl,
                        member.as_str(),
                        Namespace::Callable,
                    )
                })
            else {
                continue;
            };
            let canonical = selected.canonical(db);
            for decl in bindings::declarations_named_at(db, at, member.clone(), Namespace::Callable)
            {
                let Some(def) = crate::def_at_declaration_in_namespace(
                    db,
                    decl,
                    member.as_str(),
                    Namespace::Callable,
                ) else {
                    continue;
                };
                if !matches!(
                    def,
                    crate::Def::Function(_)
                        | crate::Def::Mapping(_)
                        | crate::Def::ScatteredClause(_)
                ) || def.canonical(db) != canonical
                {
                    continue;
                }
                let Some(node) = source_order::node(db, decl) else { continue };
                let parsed =
                    syntax::cst_lower::parsed_definition_from_cst(&node, decl.file.text(db));
                for head in &parsed.callable_heads {
                    if head.name == member.as_str()
                        && node.text_range().contains((head.label_span.start as u32).into())
                    {
                        result.push(
                            head.params
                                .iter()
                                .map(|param| CallableParameter {
                                    name: param.name.clone().unwrap_or_else(|| "_".into()),
                                    source: param.name_span.map(|span| crate::FileRange {
                                        file_id: decl.file.file_id(db),
                                        range: base_db::text_range(span.start, span.end),
                                    }),
                                })
                                .collect(),
                        );
                    }
                }
            }
        }
        Some(result)
    }

    /// Position-aware overload expansion. `None` preserves external lookup
    /// for an ad-hoc buffer; an empty result in a project is authoritative.
    pub fn callable_signatures_at(
        &self,
        file: FileText,
        offset: usize,
        name: &str,
    ) -> Option<Vec<hir_def::callable_info::CallableSignature>> {
        use hir_def::callable_info::CallableSignature;
        let db = self.db;
        use hir_def::source_order::{
            self,
            bindings::{self, Namespace},
        };
        source_order::declarations_for(db, file)?;
        let local = [offset, offset.saturating_sub(1)].into_iter().find_map(|at| {
            match self.resolve_path(db, file, at, name) {
                Some(crate::PathResolution::Local(local)) => Some(local),
                _ => None,
            }
        });
        // Incomplete code can bind a local under a global callable's name.
        // It must not display the global signature as that local's signature.
        if local.is_some() {
            return Some(Vec::new());
        }
        let Some(at) = source_order::at_offset(db, file, offset)
            .or_else(|| source_order::at_offset(db, file, offset.saturating_sub(1)))
        else {
            return Some(Vec::new());
        };
        if matches!(source_order::program(db, file), Some(source_order::Program::Inline(_)))
            && !bindings::contains_name(db, at, &hir_def::Name::new(name))
        {
            return None; // Preserve external workspace lookup for an ad-hoc buffer.
        }
        fn collect(
            db: &dyn salsa::Database,
            at: source_order::Declaration,
            name: &str,
            seen: &mut std::collections::HashSet<String>,
            out: &mut Vec<CallableSignature>,
        ) {
            if !seen.insert(name.to_owned()) {
                return;
            }
            let Some(decl) =
                bindings::resolve(db, at, hir_def::Name::new(name), Namespace::Callable)
            else {
                return;
            };
            let Some(node) = source_order::node(db, decl) else { return };
            if let Some(members) = bindings::overload_leaf_members(db, at, hir_def::Name::new(name))
            {
                for member in members {
                    collect(db, at, member.as_str(), seen, out);
                }
                return;
            }
            let definition =
                crate::def_at_declaration_in_namespace(db, decl, name, Namespace::Callable);
            let generated = definition.as_ref().is_some_and(|def| {
                def.location()
                    .item(db)
                    .is_some_and(|(tree, item)| tree.generated.contains_key(&item))
            });
            if generated || node.kind() == parser::SyntaxKind::NAMED_DEF {
                if let Some(signature) =
                    definition.and_then(|def| def.location().lookup_signature(db))
                {
                    let (root, _) = syntax::parse_text(&signature);
                    let parsed = syntax::cst_lower::parsed_file_from_cst(&root, &signature);
                    out.extend(hir_def::callable_info::collect_callable_signatures_from(
                        &parsed, &signature,
                    ));
                }
                return;
            }
            let selected = syntax::cst_lower::parsed_definition_from_cst(&node, decl.file.text(db));
            out.extend(hir_def::callable_info::collect_callable_signatures_from(
                &selected,
                decl.file.text(db),
            ));
        }
        let mut result = Vec::new();
        collect(db, at, name, &mut Default::default(), &mut result);
        Some(result)
    }

    /// Metadata for legacy/ad-hoc completion. Candidate visibility is checked
    /// by the caller; declaration selection and signature lookup live here.
    pub fn completion_details(&self, files: &[FileText], name: &str) -> CompletionDetails {
        let mut details = CompletionDetails::default();
        let mut seen = std::collections::HashSet::new();
        for &file in files {
            self.db.unwind_if_revision_cancelled();
            if !details.has_definition {
                if let Some(tree) = hir_def::def_query::file_item_tree(self.db, file).as_deref() {
                    if let Some(item) =
                        tree.top_level_items().iter().find(|item| item.name(tree).as_str() == name)
                    {
                        details.has_definition = true;
                        details.documentation = item.doc(tree).map(str::to_owned);
                    }
                }
            }
            if let Some(signature) = self.signature_index(file).get(name) {
                if seen.insert(signature.label.clone()) {
                    details.signatures.push(signature.clone());
                }
            }
        }
        details
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
        let bodies = hir_def::def_query::callable_bodies_at_offset(db, file_text, offset)?;
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

    pub fn completion_parameter_goals(
        &self,
        file: FileText,
        offset: usize,
        expected: &Ty,
        name: &str,
        fuel: &mut usize,
    ) -> Vec<Vec<Option<Ty>>> {
        if *fuel == 0 || !self.completion_callable_available(file, offset, name) {
            return Vec::new();
        }
        hir_ty::infer::completion::parameters_for_result_at(
            self.db, file, offset, expected, name, fuel,
        )
    }

    pub fn completion_application_matches(
        &self,
        file: FileText,
        offset: usize,
        expected: &Ty,
        name: &str,
        arguments: &[Ty],
        fuel: &mut usize,
    ) -> bool {
        if *fuel == 0 || !self.completion_callable_available(file, offset, name) {
            return false;
        }
        hir_ty::infer::completion::application_matches_at(
            self.db, file, offset, expected, name, arguments, fuel,
        )
    }

    fn completion_callable_available(&self, file: FileText, offset: usize, name: &str) -> bool {
        if hir_def::source_order::program(self.db, file).is_some() {
            return self.resolve_callable_at(file, offset, name).is_some();
        }
        self.imp
            .analyze(file, offset, false)
            .and_then(|sa| sa.local_at(self.db, file, offset, name))
            .is_none()
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

    /// The type of this particular call, distinct from the declaration's
    /// universally quantified scheme. Inference omits ambiguous, unresolved
    /// and unproved applications rather than synthesizing substitutions from
    /// argument text.
    pub fn call_instantiation(
        &self,
        file: FileText,
        offset: usize,
    ) -> Option<hir_ty::infer::CallInstantiation> {
        // Most hover targets are not callees. Keep those queries free of
        // inference, including declaration and local-binding positions.
        let bodies = hir_def::def_query::callable_bodies_at_offset(self.db, file, offset)?;
        let owner = bodies.0.entry_at_offset(offset)?;
        let callee_id = owner.source_map.expr_at_offset(offset)?;
        if !owner.body.iter_exprs().any(
            |(_, expr)| matches!(expr, hir_def::Expr::Call { callee, .. } if *callee == callee_id),
        ) {
            return None;
        }
        let sa = self.imp.analyze(file, offset, true)?;
        sa.infer()?.call_instantiations.get(&sa.expr_id(offset)?).cloned()
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
    /// Resolve the owning body before inference: arena IDs are local to a body
    /// and must never be looked up in another callable's inference result.
    pub fn expr_type_text(&self, file_text: FileText, span: parser::Span) -> Option<String> {
        let (_, ty) = self.expr_type_data(file_text, span)?;
        Some(ty.display_text())
    }

    /// Editor presentation of an exact expression, preserving source maps and
    /// hiding only inference-private witnesses as in binding type hints.
    pub fn expr_type_display(&self, file_text: FileText, span: parser::Span) -> Option<String> {
        let (result, ty) = self.expr_type_data(file_text, span)?;
        if matches!(ty.kind(), hir_ty::ty::TyKind::Error | hir_ty::ty::TyKind::Infer(_)) {
            return None;
        }
        Some(result.display_type(&ty))
    }

    fn expr_type_data(
        &self,
        file_text: FileText,
        span: parser::Span,
    ) -> Option<(Arc<hir_ty::InferenceResult>, Ty)> {
        let bodies = hir_def::def_query::callable_bodies_at_offset(self.db, file_text, span.start)?;
        let owner = bodies.0.entry_at_offset(span.start)?;
        let expr = owner.body.iter_exprs().find_map(|(id, _)| {
            let location = owner.source_map.expr_syntax(id)?;
            (location.start == span.start
                && (location.end == span.end || location.end == span.end.saturating_add(1)))
            .then_some(id)
        })?;
        let result = self.imp.infer_for_callable(self.db, file_text, &bodies.0, owner);
        let ty = result.expr_ty(expr)?.clone();
        Some((result, ty))
    }

    /// The inferred type of the binding at `span`, as text.
    pub fn binding_type_text(&self, file_text: FileText, span: parser::Span) -> Option<String> {
        self.binding_type_data(file_text, span).map(|(_, ty)| ty.display_text())
    }

    /// Editor presentation of a binding, without exposing a private runtime
    /// integer witness. Source generation retains the precise type-text API.
    pub fn binding_type_display(&self, file: FileText, span: parser::Span) -> Option<String> {
        self.binding_type_data(file, span).map(|(result, ty)| result.display_type(&ty))
    }

    fn binding_type_data(
        &self,
        file_text: FileText,
        span: parser::Span,
    ) -> Option<(Arc<hir_ty::InferenceResult>, Ty)> {
        let bodies = hir_def::def_query::callable_bodies_at_offset(self.db, file_text, span.start)?;
        let owner = bodies.0.entry_at_offset(span.start)?;
        let pat = owner.body.iter_pats().find_map(|(id, _)| {
            let location = owner.source_map.pat_syntax(id)?;
            (location == span).then_some(id)
        })?;
        let result = self.imp.infer_for_callable(self.db, file_text, &bodies.0, owner);
        let ty = result.pat_ty(pat)?.clone();
        Some((result, ty))
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

/// Semantic completion metadata, independent of the editor's Markdown renderer.
#[derive(Debug, Default)]
pub struct CompletionDetails {
    pub has_definition: bool,
    pub documentation: Option<String>,
    pub signatures: Vec<hir_def::callable_info::CallableSignature>,
}

#[cfg(test)]
mod span_type_tests {
    use super::*;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }
    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[test]
    fn call_types_come_from_confirmed_semantic_instances() {
        for (source, callee, expected, target) in [
            ("val identity : forall ('a : Type). 'a -> 'a\nfunction demo(x : string) -> string = identity(x)\n", "identity", "string -> string", "identity"),
            ("default Order dec\nval widen : forall 'n 'm, 'm >= 'n. (implicit('m), bits('n)) -> bits('m)\nfunction demo(x : bits(4)) -> bits(8) = widen(x)\n", "widen", "(implicit(8), bits(4)) -> bits(8)", "widen"),
            ("val keep : forall ('p : Bool). bool('p) -> bool('p)\nfunction demo(x : bool(true)) -> bool(true) = keep(x)\n", "keep", "bool(true) -> bool(true)", "keep"),
            ("default Order dec\nval keep_order : forall ('n : Nat) ('o : Order). vector('n, 'o, int) -> vector('n, 'o, int)\nfunction demo(x : vector(4, dec, int)) -> vector(4, inc, int) = keep_order(x)\n", "keep_order", "vector(4, int) -> vector(4, int)", "keep_order"),
            ("val int_case : int -> int\nval text_case : string -> string\noverload choose = {int_case, text_case}\nfunction demo(x : string) -> string = choose(x)\n", "choose", "string -> string", "text_case"),
        ] {
            let mut db = TestDb::default();
            let file = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
            hir_def::def_query::set_workspace_files(&mut db, vec![file]);
            let sema = Semantics::new(&db);
            let offset = source.rfind(&format!("{callee}(x)")).unwrap();
            let call = sema.call_instantiation(file, offset).unwrap_or_else(|| panic!("no instance for {source}: {:?}", sema.imp.analyze(file, offset, true).unwrap().infer()));
            assert_eq!(call.ty.display_text(), expected, "{source}");
            assert_eq!(call.target, target, "{source}");
            if callee == "widen" { assert_eq!(call.implicit_params, [true, false]); }
            // A declaration and an argument are not call-type targets.
            assert!(sema.call_instantiation(file, source.find(callee).unwrap()).is_none());
            assert!(sema.call_instantiation(file, offset + callee.len() + 1).is_none());
        }
    }

    #[test]
    fn unresolved_and_ambiguous_calls_do_not_publish_specializations() {
        for (source, callee) in [
            ("val unknown : forall ('a : Type). unit -> 'a\nfunction demo() = unknown()\n", "unknown"),
            ("val first : int -> int\nval second : int -> int\noverload choose = {first, second}\nfunction demo(x : int) -> int = choose(x)\n", "choose"),
            ("val need : int -> int\nfunction demo() -> int = need(true)\n", "need"),
            ("val dependent : forall ('a : Type). 'a -> {'a, 'a > 0. bits('a)}\nfunction demo(x : string) = dependent(x)\n", "dependent"),
        ] {
            let mut db = TestDb::default();
            let file = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
            hir_def::def_query::set_workspace_files(&mut db, vec![file]);
            let offset = source.rfind(&format!("{callee}(")).unwrap();
            assert!(Semantics::new(&db).call_instantiation(file, offset).is_none(), "{source}");
        }
    }

    #[test]
    fn position_queries_share_arenas_and_keep_current_view_source_maps() {
        use crate::source_analyzer::BodyAnalysis;
        use salsa::Setter;
        let mut db = TestDb::default();
        let source = "function first(x : int) -> int = x\n";
        // A semantic include view uses identical physical text, but must
        // retain its own provenance in every syntax pointer.
        let physical = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
        let view = FileText::new(&db, Arc::from(source), FileId::for_instance(0));
        let (old_body, old_map, weak) = {
            let sema = Semantics::new(&db);
            let offset = source.rfind('x').unwrap();
            let first = sema.imp.analyze(view, offset, false).unwrap();
            let second = sema.imp.analyze(view, offset - 1, false).unwrap();
            let cached = sema.imp.analyze(view, offset - 1, false).unwrap();
            let Some(BodyAnalysis { body, source_map, .. }) = &first.body else {
                panic!("expected callable");
            };
            let lowered = hir_def::def_query::callable_bodies(&db, view).as_ref().unwrap();
            assert!(Arc::ptr_eq(body, &lowered.0.entries()[0].body));
            assert!(Arc::ptr_eq(source_map, &lowered.0.entries()[0].source_map));
            let id = hir_def::def_query::DefWithBodyId::new(&db, view, "first".to_owned(), 0);
            let query = hir_def::def_query::body_with_source_map(&db, id);
            assert!(Arc::ptr_eq(body, &query.0 .0));
            assert!(Arc::ptr_eq(&source_map.store, &query.0 .1.store));
            for analyzer in [&second, &cached] {
                let Some(BodyAnalysis { body: other, source_map: map, .. }) = &analyzer.body else {
                    panic!("expected callable");
                };
                assert!(Arc::ptr_eq(body, other));
                assert!(Arc::ptr_eq(source_map, map));
            }
            assert_eq!(source_map.file_id, Some(FileId::for_instance(0)));
            let physical_analyzer = sema.imp.analyze(physical, offset, false).unwrap();
            assert_eq!(physical_analyzer.source_map().unwrap().file_id, Some(FileId::from_raw(0)));
            (body.clone(), source_map.clone(), Arc::downgrade(source_map))
        };
        let edited = format!("// moved\n{source}");
        view.set_text(&mut db).to(Arc::from(edited.as_str()));
        {
            let sema = Semantics::new(&db);
            let current = sema.imp.analyze(view, edited.rfind('x').unwrap(), false).unwrap();
            let Some(BodyAnalysis { body, source_map, .. }) = &current.body else {
                panic!("expected callable");
            };
            assert!(!Arc::ptr_eq(&old_body, body));
            assert!(!Arc::ptr_eq(&old_map, source_map));
            let before = old_map.expr_at_offset(source.rfind('x').unwrap()).unwrap();
            let after = source_map.expr_at_offset(edited.rfind('x').unwrap()).unwrap();
            assert_eq!(
                source_map.expr_syntax(after).unwrap().start,
                old_map.expr_syntax(before).unwrap().start + 9
            );
        }
        drop(old_map);
        drop(old_body);
        drop(db);
        assert!(weak.upgrade().is_none(), "old view maps must be releasable");
    }

    #[test]
    fn span_type_queries_do_not_reuse_another_bodys_arena_ids() {
        let mut db = TestDb::default();
        let source =
            "function first(x : int) -> int = x\nfunction second(word : string) -> string = word\n";
        let file = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file]);
        let sema = Semantics::new(&db);
        let expression = source.rfind("word").unwrap();
        assert_eq!(
            sema.expr_type_text(file, parser::Span::new(expression, expression + 4)),
            Some("string".into())
        );
        let binding = source.find("word").unwrap();
        assert_eq!(
            sema.binding_type_text(file, parser::Span::new(binding, binding + 4)),
            Some("string".into())
        );
        assert!(sema.expr_type_text(file, parser::Span::new(0, 8)).is_none());
    }
}

mod explanations;
pub use explanations::{
    CandidateExplanation, ConstraintExplanation, ExpressionExplanation, MismatchExplanation,
};
