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
    /// - `crate_def_map` for building the resolver
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

        // Find the callable containing this offset
        let callable = bodies.0.entry_at_offset(offset)?;

        // Build DefMap from salsa-cached query — Resolver derived on demand
        let def_map_arc = hir_def::def_query::crate_def_map(db, file_text).as_ref()?;
        let resolver = def_map_arc.0.clone();

        let body = (*callable.body).clone();
        let mut source_map = (*callable.source_map).clone();
        // Ensure file_id is set (does this in callable_bodies,
        // but callable_bodies may not have it)
        if source_map.file_id.is_none() {
            source_map.file_id = Some(file_id);
        }

        let containing_callable_name = Some(callable.name.clone());

        if infer {
            // Per-callable inference via salsa
            let inference = self.infer_for_callable(db, file_text, &bodies.0, callable);
            Some(SourceAnalyzer {
                file_id,
                resolver,
                body_or_sig: Some(crate::source_analyzer::BodyOrSig::Body {
                    body,
                    source_map,
                    infer: inference,
                }),
                containing_callable_name,
            })
        } else {
            Some(SourceAnalyzer {
                file_id,
                resolver,
                body_or_sig: Some(crate::source_analyzer::BodyOrSig::Body {
                    body,
                    source_map,
                    infer: None,
                }),
                containing_callable_name,
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

    /// Create a SourceAnalyzer for a position (with inference).
    ///
    /// Takes `FileText` (salsa input) instead of raw source string.
    pub fn analyze(&self, file_text: FileText, offset: usize) -> Option<SourceAnalyzer> {
        self.imp.analyze(file_text, offset, true)
    }

    /// Create a SourceAnalyzer without inference (faster, for navigation-only).
    pub fn analyze_no_infer(&self, file_text: FileText, offset: usize) -> Option<SourceAnalyzer> {
        self.imp.analyze(file_text, offset, false)
    }

    /// Get the type of an expression at an offset.
    pub fn type_of_expr(&self, file_text: FileText, offset: usize) -> Option<Ty> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.type_of_expr(expr_id).cloned()
    }

    /// Get the type of a pattern at an offset.
    pub fn type_of_pat(&self, file_text: FileText, offset: usize) -> Option<Ty> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let pat_id = sa.pat_id(offset)?;
        sa.type_of_pat(pat_id).cloned()
    }

    /// Resolve a name at position to a `PathResolution`.
    /// Takes `db: &dyn DefDatabase` ( — all semantic
    /// queries flow through the database).
    pub fn resolve_path(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        file_text: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let sa = self.imp.analyze(file_text, offset, false)?;
        sa.resolve_path(db, name)
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

    /// Returns the function containing the given position, if any.
    ///
    /// In Sail, "containing function" means the callable whose body
    /// encloses the offset.
    pub fn containing_function(
        &self,
        file_text: FileText,
        offset: usize,
    ) -> Option<crate::Function> {
        let sa = self.imp.analyze(file_text, offset, false)?;
        let callable_name = sa.containing_callable_name.as_ref()?;

        // Look up the Function in the DefMap by name
        let def_map_arc = hir_def::def_query::crate_def_map(self.db, file_text).as_ref()?;
        let def_map = &def_map_arc.0;
        for (idx, entry) in def_map.entries().iter().enumerate() {
            if entry.name.as_str() == callable_name.as_str()
                && matches!(
                    entry.kind,
                    hir_def::item_tree::ItemKind::Function | hir_def::item_tree::ItemKind::Mapping
                )
            {
                let def_id = hir_def::ModuleDefId::from_def_id(
                    hir_def::nameres::DefId(idx as u32),
                    entry.kind,
                );
                let loc = crate::DefLocation { file_text, def_id };
                return Some(crate::Function { id: loc });
            }
        }
        None
    }

    /// Returns a `NameRefKind` that classifies whether the name reference is
    /// a plain path, a field access (preceded by `.`), or a function call
    /// (followed by `(`).
    pub fn classify_name_ref(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        file_text: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::NameRefKind> {
        let source = file_text.text(self.db);

        // Determine syntactic context by scanning characters around the name
        let is_field_access = {
            // Walk backwards from offset to find a preceding '.'
            let before = &source[..offset];
            let trimmed = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
            trimmed.ends_with('.')
        };
        let is_call = {
            // Walk forwards past the name to find a following '('
            let name_end = offset + name.len();
            if name_end <= source.len() {
                let after = &source[name_end..];
                let trimmed = after.trim_start();
                trimmed.starts_with('(')
            } else {
                false
            }
        };

        if is_field_access {
            return Some(crate::NameRefKind::FieldAccess { field_name: name.to_string() });
        }

        // Try semantic resolution
        let sa = self.imp.analyze(file_text, offset, true);
        let resolution = sa.as_ref().and_then(|sa| sa.resolve_path(db, name));

        if is_call {
            return Some(crate::NameRefKind::Call {
                callee_name: name.to_string(),
                resolution,
            });
        }

        match resolution {
            Some(res) => Some(crate::NameRefKind::Path(res)),
            None => Some(crate::NameRefKind::Unresolved),
        }
    }

    /// Legacy classify_name_ref that returns just a PathResolution.
    /// Used by callers that only need the resolution, not the kind.
    pub fn classify_name_ref_path(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        file_text: FileText,
        offset: usize,
        name: &str,
    ) -> Option<crate::PathResolution> {
        match self.classify_name_ref(db, file_text, offset, name)? {
            crate::NameRefKind::Path(res) => Some(res),
            crate::NameRefKind::Call { resolution, .. } => resolution,
            crate::NameRefKind::FieldAccess { .. } => None,
            crate::NameRefKind::Unresolved => None,
        }
    }

    /// 投産-4: Resolve a field access expression at the given offset.
    /// Returns the DefId of the struct field definition, if the expression
    /// at `offset` is a field access and inference data is available.
    pub fn resolve_field(
        &self,
        file_text: FileText,
        offset: usize,
    ) -> Option<hir_def::ModuleDefId> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.resolve_field(expr_id)
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
    ) -> Option<either::Either<hir_def::ModuleDefId, (hir_def::item_id::FunctionId, base_db::FileId)>> {
        let sa = self.imp.analyze(file_text, offset, true)?;
        let expr_id = sa.expr_id(offset)?;
        sa.resolve_field_or_method(expr_id)
    }

    /// Collect all names visible in the scope at the given offset.
    ///
    /// wraps the same call through a public API on Semantics so that
    /// external crates (ide-completion) can access it without reaching
    /// into `pub(crate)` internals.
    pub fn names_in_scope(
        &self,
        file_text: FileText,
        offset: usize,
    ) -> Vec<hir_def::Name> {
        let sa = match self.imp.analyze(file_text, offset, false) {
            Some(sa) => sa,
            None => return Vec::new(),
        };
        let resolver = sa.make_resolver();
        resolver.names_in_scope()
    }

    /// Debug: return the pretty-printed HIR body at the given position.
    pub fn debug_hir_at(&self, file_text: FileText, offset: usize) -> Option<String> {
        let sa = self.imp.analyze(file_text, offset, false)?;
        let body = sa.body()?;
        Some(hir_def::expr_store::pretty::print_body(body))
    }

    /// Get the scope at a given offset.
    /// Uses salsa-cached DefMap.
    pub fn scope_at_offset(&self, file_text: FileText, _offset: usize) -> Option<SemanticsScope> {
        let db = self.db;
        let file_id = file_text.file_id(db);
        let def_map_arc = hir_def::def_query::crate_def_map(db, file_text).as_ref()?;
        Some(SemanticsScope { file_id, resolver: def_map_arc.0.clone() })
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

    /// Iterate over all names visible in this scope.
    pub fn process_all_names(&self, f: &mut dyn FnMut(hir_def::Name, ScopeDef)) {
        use hir_def::item_tree::ItemKind;

        let def_map = &self.resolver;
        for entry in def_map.entries() {
            let scope_def = match entry.kind {
                ItemKind::Function
                | ItemKind::Mapping
                | ItemKind::ValSpec
                | ItemKind::MappingSpec
                | ItemKind::Let
                | ItemKind::Var
                | ItemKind::Overload => ScopeDef::ModuleDef(entry.name.clone()),
                ItemKind::Struct
                | ItemKind::Union
                | ItemKind::Enum
                | ItemKind::Bitfield
                | ItemKind::Newtype
                | ItemKind::TypeAlias => ScopeDef::ModuleDef(entry.name.clone()),
                ItemKind::Register => ScopeDef::ModuleDef(entry.name.clone()),
                _ => continue,
            };
            f(entry.name.clone(), scope_def);
        }
    }
}

/// What can appear in a scope.
///
/// Simplified for Sail (no const generics, no loop labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeDef {
    /// A top-level definition (function, type, register, etc.).
    ModuleDef(hir_def::Name),
    /// A local variable binding.
    Local(crate::Local),
    /// A type variable.
    GenericParam(crate::GenericParam),
    /// Unknown / unresolved.
    Unknown,
}

impl SemanticsScope {
    /// Iterate all visible definitions as (Name, ScopeDef) pairs.
    ///
    /// More convenient than `process_all_names` for collecting into a Vec.
    pub fn visible_defs(&self) -> Vec<(hir_def::Name, ScopeDef)> {
        let mut defs = Vec::new();
        self.process_all_names(&mut |name, scope_def| {
            defs.push((name, scope_def));
        });
        defs
    }

    /// Resolve a name in this scope.
    ///
    /// Takes `db``).
    pub fn resolve_name(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        name: &str,
    ) -> Option<crate::PathResolution> {
        let resolver = self.make_resolver();
        if let Some(type_ns) = resolver.resolve_path_in_type_ns(db, name) {
            return Some(crate::PathResolution::from_type_ns(type_ns));
        }
        if let Some(value_ns) = resolver.resolve_path_in_value_ns(db, name) {
            return Some(crate::PathResolution::from_value_ns(value_ns));
        }
        None
    }
}
