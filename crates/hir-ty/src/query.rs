//! Salsa tracked queries for type inference.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base_db::FileText;

use std::hash::Hasher;

use std::collections::BTreeSet;

use crate::infer::{InferenceResult, TopLevelEnv, TypeCheckResult};

/// Newtype wrapper for (TopLevelEnv, HashSet<String>) with pointer-based
/// Eq/Hash for salsa tracked query return values.
#[derive(Clone, Debug)]
pub struct ArcTopLevelEnv(pub Arc<TopLevelEnvData>);

/// The actual data stored inside the Arc.
#[derive(Clone, Debug)]
pub struct TopLevelEnvData {
    pub env: TopLevelEnv,
    pub pattern_constants: HashSet<String>,
    /// Content-based fingerprint of signature data only.
    ///
    /// When a function body changes but signatures don't, this
    /// fingerprint stays the same. Salsa's equality check on the query
    /// return value then skips all downstream re-computation.
    pub signatures_fingerprint: u64,
}

impl PartialEq for ArcTopLevelEnv {
    fn eq(&self, other: &Self) -> bool {
        self.0.signatures_fingerprint == other.0.signatures_fingerprint
    }
}
impl Eq for ArcTopLevelEnv {}
impl std::hash::Hash for ArcTopLevelEnv {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.signatures_fingerprint.hash(state);
    }
}

/// Per-file TopLevelEnv built from CST.
///
/// Uses content-based signature fingerprint for equality so that
/// body-only edits skip downstream re-computation.
#[salsa::tracked(returns(ref))]
pub fn top_level_env(db: &dyn salsa::Database, input: FileText) -> ArcTopLevelEnv {
    let text: &str = &input.text(db);
    let parsed = syntax::parse_query::parse_file(db, input);
    let cst_root = match &parsed.green {
        Some(green) => syntax::SyntaxNode::new_root(green.as_ref().clone()),
        None => {
            return ArcTopLevelEnv(Arc::new(TopLevelEnvData {
                env: TopLevelEnv::default(),
                pattern_constants: Default::default(),
                signatures_fingerprint: 0,
            }))
        }
    };
    let (mut env, pattern_constants) = TopLevelEnv::from_cst(&cst_root);
    let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
    super::infer::env::apply_callable_signature_metadata(&parsed_file, text, &mut env);
    let signatures_fingerprint = compute_signatures_fingerprint(&env);
    ArcTopLevelEnv(Arc::new(TopLevelEnvData { env, pattern_constants, signatures_fingerprint }))
}

/// Compute a content-based fingerprint of the signature data in a
/// TopLevelEnv. Only hashes data from val specs and type definitions,
/// NOT from function bodies. If signatures haven't changed, salsa's
/// equality check prevents re-running downstream queries.
fn compute_signatures_fingerprint(env: &TopLevelEnv) -> u64 {
    use std::hash::Hash;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hash function signatures (name + scheme count + param count per scheme)
    let mut fn_names: Vec<&String> = env.functions.keys().collect();
    fn_names.sort();
    for name in fn_names {
        name.hash(&mut hasher);
        if let Some(schemes) = env.functions.get(name) {
            schemes.len().hash(&mut hasher);
            for scheme in schemes {
                scheme.params.len().hash(&mut hasher);
                scheme.ret.hash(&mut hasher);
                scheme.quantifiers.hash(&mut hasher);
            }
        }
    }
    // Hash type definitions (enums, unions, records, type aliases)
    let mut enum_names: Vec<&String> = env.enums.keys().collect();
    enum_names.sort();
    for name in enum_names {
        name.hash(&mut hasher);
        if let Some(members) = env.enums.get(name) {
            members.hash(&mut hasher);
        }
    }
    let mut union_names: Vec<&String> = env.unions.keys().collect();
    union_names.sort();
    for name in union_names {
        name.hash(&mut hasher);
        if let Some(variants) = env.unions.get(name) {
            variants.hash(&mut hasher);
        }
    }
    let mut record_names: Vec<&String> = env.records.keys().collect();
    record_names.sort();
    for name in record_names {
        name.hash(&mut hasher);
        if let Some(info) = env.records.get(name) {
            info.params.hash(&mut hasher);
            // Hash field names (sorted for determinism)
            let mut field_names: Vec<&String> = info.fields.keys().collect();
            field_names.sort();
            for fname in field_names {
                fname.hash(&mut hasher);
            }
        }
    }
    // Hash constructor signatures
    let mut ctor_names: Vec<&String> = env.constructors.keys().collect();
    ctor_names.sort();
    for name in ctor_names {
        name.hash(&mut hasher);
        if let Some(schemes) = env.constructors.get(name) {
            schemes.len().hash(&mut hasher);
        }
    }
    // Hash mapping signatures
    let mut map_names: Vec<&String> = env.mappings.keys().collect();
    map_names.sort();
    for name in map_names {
        name.hash(&mut hasher);
        if let Some(schemes) = env.mappings.get(name) {
            schemes.len().hash(&mut hasher);
        }
    }
    // Hash type aliases
    let mut alias_names: Vec<&String> = env.type_aliases.keys().collect();
    alias_names.sort();
    for name in alias_names {
        name.hash(&mut hasher);
    }
    hasher.finish()
}

/// Newtype wrapper for `WorkspaceContext` with content-based Eq/Hash.
///
/// Uses `signatures_fingerprint` for equality — body-only edits
/// produce the same fingerprint, preventing downstream `infer`
/// re-runs.
#[derive(Clone, Debug)]
pub struct ArcWorkspaceContext(pub Arc<crate::infer::WorkspaceContext>);

impl PartialEq for ArcWorkspaceContext {
    fn eq(&self, other: &Self) -> bool {
        self.0.signatures_fingerprint == other.0.signatures_fingerprint
    }
}
impl Eq for ArcWorkspaceContext {}
impl std::hash::Hash for ArcWorkspaceContext {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.signatures_fingerprint.hash(state);
    }
}

/// Workspace-wide cross-file type data query.
///
/// - Takes `WorkspaceFiles` (salsa singleton input listing all files)
/// - Iterates all files' `top_level_env` + `file_item_tree`
/// - Aggregates cross-file type aliases, overloads, records, name sets
/// - Returns `ArcWorkspaceContext` with content-based Eq via fingerprint
///
/// salsa dependency chain:
///   `infer` → `workspace_context` → `top_level_env` × N files
///
/// Body-only edits: `top_level_env` re-runs but `ArcTopLevelEnv::eq`
/// returns true (same `signatures_fingerprint`) → this query is NOT re-run.
///
/// Signature edits: `ArcTopLevelEnv::eq` returns false → this query
/// re-runs → `ArcWorkspaceContext` may change → `infer` re-runs.
#[salsa::tracked(returns(ref))]
pub fn workspace_context(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
) -> ArcWorkspaceContext {
    let _p = tracing::info_span!("workspace_context").entered();
    let mut ctx = crate::infer::WorkspaceContext::default();
    for &ft in ws.file_texts(db) {
        let env_data = top_level_env(db, ft);
        let item_tree = hir_def::def_query::file_item_tree(db, ft);
        ctx.merge_file_from_queries(&env_data.0, item_tree.as_ref(), ft);
    }
    ctx.compute_fingerprints();
    tracing::debug!(
        functions = ctx.cross_file_function_names().len(),
        constructors = ctx.cross_file_constructor_names().len(),
        type_aliases = ctx.type_aliases_count(),
        fingerprint = ctx.signatures_fingerprint,
        "workspace context built"
    );
    ArcWorkspaceContext(Arc::new(ctx))
}

#[derive(Clone, Debug)]
pub struct ArcInferenceResult(pub Arc<InferenceResult>);

impl PartialEq for ArcInferenceResult {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcInferenceResult {}
impl std::hash::Hash for ArcInferenceResult {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

/// Per-callable InferenceResult query.
///
/// Returns the InferenceResult directly, suitable for SourceAnalyzer
/// and other consumers that need per-expression type information.
/// No LRU — salsa retains all results until GC.
#[salsa::tracked(returns(ref))]
pub fn infer_for_body<'db>(
    db: &'db dyn salsa::Database,
    id: hir_def::def_query::DefWithBodyId<'db>,
) -> ArcInferenceResult {
    // Delegate to infer which already does the work
    let tc = infer(db, id);
    ArcInferenceResult(Arc::new(tc.0.as_ref().clone()))
}

/// Per-file diagnostics collection from per-callable inference.
///
/// Does NOT merge InferenceResult across callables -- only collects
/// diagnostics for workspace-level diagnostic push. Consumers that
/// need per-callable type info should query `infer` directly.
#[salsa::tracked(returns(ref))]
pub fn infer_body(db: &dyn salsa::Database, input: FileText) -> ArcInferenceResult {
    let callable_ids = hir_def::def_query::file_def_with_body_ids(db, input);
    let mut all_diagnostics = Vec::new();

    for &id in callable_ids {
        db.unwind_if_revision_cancelled();
        let result = infer(db, id);
        all_diagnostics.extend(result.0.diagnostics().iter().cloned());
    }

    let mut result = TypeCheckResult::default();
    result.legacy_diagnostics = all_diagnostics;
    ArcInferenceResult(Arc::new(result))
}

/// Per-callable inference query (salsa-tracked).
///
/// Finest-grained inference query: editing one function body only
/// re-infers that function. No LRU -- salsa retains all results
/// until GC.
#[salsa::tracked(returns(ref))]
pub fn infer<'db>(
    db: &'db dyn salsa::Database,
    id: hir_def::def_query::DefWithBodyId<'db>,
) -> ArcInferenceResult {
    let _p = tracing::info_span!("infer", name = %id.name(db)).entered();

    let file = id.file(db);
    let text: &str = &file.text(db);

    let env_data = top_level_env(db, file);
    let mut env = env_data.0.env.clone();
    let mut pattern_constants = env_data.0.pattern_constants.clone();

    tracing::debug!(
        local_type_aliases = env.type_aliases.len(),
        local_functions = env.functions.len(),
        local_overloads = env.overloads.len(),
        "per-file env before workspace merge"
    );

    // Apply workspace context via salsa tracked query.
    //
    // `infer` depends on `workspace_context` which depends on
    // `top_level_env` for all files. salsa handles caching and
    // invalidation automatically — no global static, no manual timing.
    if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
        let ws_ctx = workspace_context(db, ws);
        ws_ctx.0.apply_to(&mut env, &mut pattern_constants);
        // NOTE: has_workspace_context NOT set — strict unresolved-ident
        // checking produces thousands of false positives.

        tracing::debug!(
            merged_type_aliases = env.type_aliases.len(),
            merged_functions = env.functions.len(),
            merged_overloads = env.overloads.len(),
            has_cross_file_schemes = env.cross_file_schemes.is_some(),
            "env after workspace merge"
        );
    } else {
        tracing::debug!("no WorkspaceFiles — single-file mode");
    }

    let callable_bodies = hir_def::def_query::callable_bodies(db, file);
    let Some(bodies) = callable_bodies.as_ref() else {
        return ArcInferenceResult(Arc::new(TypeCheckResult::default()));
    };

    let target_name = id.name(db);
    let target_clause = id.clause_index(db);

    db.unwind_if_revision_cancelled();

    let mut clause_idx = 0u32;
    for entry in bodies.0.entries() {
        if entry.name == target_name {
            if clause_idx == target_clause {
                let mut ctx = crate::infer::InferenceContext::new_for_body(
                    text,
                    entry.body.clone(),
                    entry.source_map.clone(),
                    env,
                    pattern_constants,
                );
                ctx.db = Some(db);
                if entry.body.mapping_arms.is_empty() {
                    ctx.infer_callable_body_hir(&entry.name, entry);
                } else {
                    ctx.infer_mapping_body_hir(&entry.name, entry);
                }
                return ArcInferenceResult(Arc::new(ctx.finish_query()));
            }
            clause_idx += 1;
        }
    }

    // Callable not found — return empty result
    ArcInferenceResult(Arc::new(TypeCheckResult::default()))
}

/// Newtype wrapper for transitive effects (per-file).
#[derive(Clone, Debug)]
pub struct ArcTransitiveEffects(pub Arc<HashMap<String, BTreeSet<hir_def::EffectTag>>>);

impl PartialEq for ArcTransitiveEffects {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcTransitiveEffects {}
impl std::hash::Hash for ArcTransitiveEffects {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

/// Per-file transitive effect computation.
///
/// Propagates effects through the call graph, so if `foo` calls `bar`
/// and `bar` has effect `throw`, then `foo` also has effect `throw`.
#[salsa::tracked(returns(ref))]
pub fn transitive_effects(db: &dyn salsa::Database, input: FileText) -> ArcTransitiveEffects {
    let mut effects: HashMap<String, BTreeSet<hir_def::EffectTag>> = HashMap::new();

    // Get declared effects from bodies
    if let Some(bodies) =
        hir_def::def_query::callable_bodies(db, input).as_ref().map(|b| b.0.as_ref())
    {
        for body in bodies.entries() {
            if !body.effects.is_empty() {
                effects.insert(body.name.clone(), body.effects.clone());
            }
        }
    }

    // Propagate through call graph
    if let Some(cg) = hir_def::def_query::callgraph(db, input).as_ref().map(|acg| acg.0.as_ref()) {
        // Fixed-point iteration
        let mut changed = true;
        while changed {
            changed = false;
            let snapshot: Vec<(String, BTreeSet<hir_def::EffectTag>)> =
                effects.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (name, _) in &snapshot {
                let callee_effects: BTreeSet<hir_def::EffectTag> = cg
                    .callees_of(name)
                    .filter_map(|callee| effects.get(callee))
                    .flat_map(|e| e.iter().copied())
                    .collect();
                if !callee_effects.is_empty() {
                    let entry = effects.entry(name.clone()).or_default();
                    let before = entry.len();
                    entry.extend(callee_effects);
                    if entry.len() > before {
                        changed = true;
                    }
                }
            }
        }
    }

    ArcTransitiveEffects(Arc::new(effects))
}

// ── Backward-compatible aliases (old `_query` names) ──────────────
// These allow downstream code that hasn't been updated yet to keep

#[cfg(test)]
mod tests {
    use super::*;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[test]
    fn top_level_env_extracts_function_scheme() {
        let db = TestDb::default();
        let text: Arc<str> = Arc::from("val add : (int, int) -> int\nfunction add(x, y) = x + y\n");
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let result = top_level_env(&db, input);
        assert!(result.0.env.functions.contains_key("add"), "expected 'add' in env.functions");
    }

    #[test]
    fn infer_body_produces_diagnostics() {
        // Verify per-callable inference produces diagnostics via
        // InferenceDiagnostic or type_mismatches pipelines.
        let db = TestDb::default();
        // MismatchedArgCount is emitted when arg count doesn't match.
        let text: Arc<str> =
            Arc::from("val f : (int, int) -> int\nfunction f(x, y) = x + y\nfunction g() = f(1)\n");
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let callable_ids = hir_def::def_query::file_def_with_body_ids(&db, input);
        let has_any_diagnostic = callable_ids.iter().any(|&id| {
            let tcr = infer(&db, id);
            tcr.0
                .inference_diagnostics()
                .iter()
                .any(|d| matches!(d, crate::infer::InferenceDiagnostic::MismatchedArgCount { .. }))
        });
        assert!(has_any_diagnostic, "expected MismatchedArgCount for f(1) when f takes 2 args");
    }

    #[test]
    fn infer_body_is_cached() {
        let db = TestDb::default();
        let text: Arc<str> = Arc::from("function f(x) = x + 1\n");
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let r1 = infer_body(&db, input);
        let r2 = infer_body(&db, input);
        assert!(Arc::ptr_eq(&r1.0, &r2.0), "query should be cached");
    }

    #[test]
    fn top_level_env_is_cached() {
        let db = TestDb::default();
        let text: Arc<str> = Arc::from("val f : int -> int\n");
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let r1 = top_level_env(&db, input);
        let r2 = top_level_env(&db, input);
        // Same salsa revision → same Arc (pointer equality)
        assert!(Arc::ptr_eq(&r1.0, &r2.0), "query should be cached");
    }

    #[test]
    fn sail_riscv_style_forall_constraint() {
        // Real sail-riscv pattern: forall with multiple constraints
        let db = TestDb::default();
        let text: Arc<str> = Arc::from(
            "val add_bits : forall 'n. (bits('n), bits('n)) -> bits('n)\n\
             function add_bits(x, y) = x\n",
        );
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let env = top_level_env(&db, input);
        assert!(
            env.0.env.functions.contains_key("add_bits"),
            "should extract add_bits from forall val spec"
        );
    }

    #[test]
    fn sail_riscv_existential_type_alias() {
        // Type alias with existential: type nat1 = {'n, 'n > 0. int('n)}
        let db = TestDb::default();
        let text: Arc<str> = Arc::from(
            "type xlenbits = bits(64)\n\
             val pc_read : unit -> xlenbits\n\
             function pc_read() = undefined\n",
        );
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let env = top_level_env(&db, input);
        assert!(
            env.0.env.type_aliases.contains_key("xlenbits"),
            "should extract xlenbits type alias"
        );
        assert!(env.0.env.functions.contains_key("pc_read"), "should extract pc_read function");
    }

    #[test]
    fn scattered_function_infers() {
        let db = TestDb::default();
        let text: Arc<str> = Arc::from(
            "scattered function execute\n\
             function clause execute(instr) = false\n\
             end execute\n",
        );
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let result = infer_body(&db, input);
        // Should not panic and should produce some result
        let _ = result.0.diagnostics().len();
    }

    #[test]
    fn sail_riscv_main_pattern() {
        // Real sail-riscv main.sail pattern with try/catch
        // Query per-callable (not per-file merge)
        let db = TestDb::default();
        let text: Arc<str> = Arc::from(
            r#"
function main() : unit -> unit = {
  try {
    let x = 1;
    x + 1
  } catch {
    _ => ()
  }
}
"#,
        );
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let callable_ids = hir_def::def_query::file_def_with_body_ids(&db, input);
        assert!(!callable_ids.is_empty(), "should have callable IDs");
        let result = infer(&db, callable_ids[0]);
        assert!(result.0.expr_count() > 0, "should have inferred expressions");
    }

    #[test]
    fn forall_with_set_constraint() {
        // Pattern: forall 'n, 'n in {16, 32}
        let db = TestDb::default();
        let text: Arc<str> = Arc::from(
            "val footprint : forall 'n, 'n in {16, 32}. bits('n) -> bool\n\
             function footprint(opcode) = true\n",
        );
        let input = FileText::new(&db, text, base_db::FileId::from_raw(0));
        let env = top_level_env(&db, input);
        assert!(
            env.0.env.functions.contains_key("footprint"),
            "should extract footprint with set constraint"
        );
        let schemes = &env.0.env.functions["footprint"];
        assert!(!schemes.is_empty(), "should have at least one scheme");
        assert!(
            !schemes[0].quantifiers.is_empty(),
            "should have quantifiers: {:?}",
            schemes[0].quantifiers
        );
    }

    /// Verify that cross-file symbols are visible during
    /// workspace type checking via the cached_workspace_context path.
    #[test]
    fn cross_file_include_symbols_visible() {
        use hir_def::callgraph::SourceFileInfo;

        // File B: defines helper
        let source_b = "val helper : int -> int\nfunction helper(x) = x + 1\n";
        // File A: uses helper (simulating $include)
        let source_a = "val main : unit -> int\nfunction main() = helper(42)\n";

        // Adapter: SourceFileInfo for raw text
        struct TextFile<'a>(&'a str);
        impl hir_def::callgraph::WorkspaceFile for TextFile<'_> {
            fn content_hash(&self) -> u64 {
                0
            }
            fn callgraph(&self) -> Option<&hir_def::callgraph::CallGraph> {
                None
            }
        }
        impl SourceFileInfo for TextFile<'_> {
            fn text(&self) -> &str {
                self.0
            }
            fn item_tree(&self) -> Option<&hir_def::ItemTree> {
                None
            }
        }

        let file_b = TextFile(source_b);
        let file_a = TextFile(source_a);

        // Workspace context is built implicitly by check_file_with_workspace below.

        // Type-check file A with workspace context
        let result = crate::infer::check_file_with_workspace(
            &file_a as &dyn SourceFileInfo,
            vec![&file_b, &file_a].into_iter(),
            true,
            crate::CancellationToken::never(),
        );

        // Should produce a result (not None)
        assert!(result.is_some(), "type check should succeed");
        let tc = result.unwrap();
        // The call to `helper(42)` should not produce an "unknown function"
        // type error IF workspace context correctly provides the name.
        // (It may still produce type inference issues since TextFile returns
        // no ItemTree, but the point is the function IS found.)
        let has_unknown_fn_error = tc
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("Unknown function") && d.message.contains("helper"));
        assert!(
            !has_unknown_fn_error,
            "helper should be visible via workspace context, got diagnostics: {:?}",
            tc.diagnostics().iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Verify AnalysisScope correctly filters files.
    #[test]
    fn analysis_scope_filters_files() {
        use hir_def::analysis_scope::AnalysisScope;
        use hir_def::include_graph::IncludeGraph;

        let mut graph = IncludeGraph::new();
        graph.add_edge(base_db::FileId::from_raw(0), base_db::FileId::from_raw(1));
        // FileId::from_raw(2) is NOT included by FileId::from_raw(0)

        let scope = AnalysisScope::from_include_graph(&graph, base_db::FileId::from_raw(0));
        assert!(scope.contains(base_db::FileId::from_raw(0)));
        assert!(scope.contains(base_db::FileId::from_raw(1)));
        assert!(
            !scope.contains(base_db::FileId::from_raw(2)),
            "FileId::from_raw(2) should NOT be in scope"
        );
    }
}
