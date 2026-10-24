//! Salsa tracked queries for type inference.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base_db::FileText;

use std::collections::BTreeSet;

use crate::infer::{InferenceResult, TopLevelEnv, TypeCheckResult};

/// Semantic query result. Equality includes every exported type, constraint,
/// effect, overload and pattern constant, but no callable bodies or offsets.
/// Salsa can backdate body-only edits without hiding signature changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcTopLevelEnv(pub Arc<TopLevelEnvData>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopLevelEnvData {
    pub env: TopLevelEnv,
    pub pattern_constants: HashSet<String>,
}

/// The file that declares the *type* `name`, as seen from `from`.
///
/// The workspace context knows which file declares what — it is how a call
/// resolves a callee declared elsewhere — and until now nothing above
/// `hir-ty` could ask it about a *type*. A feature holding a type's name and
/// wanting its declaration had to search every file or give up:
/// `classify_struct_field` gave up, so a struct literal whose record is
/// declared in another file hovered as the bare word.
pub fn declaring_type_file(
    db: &dyn salsa::Database,
    from: FileText,
    name: &str,
) -> Option<FileText> {
    let ws = base_db::WorkspaceFiles::try_get(db)?;
    crate::infer::dependencies::declaring_type_file(db, ws, from, name)
}

/// Per-file TopLevelEnv built from CST.
///
/// Uses structural semantic equality so that
/// body-only edits skip downstream re-computation.
#[salsa::tracked(returns(ref))]
pub fn top_level_env(db: &dyn salsa::Database, input: FileText) -> ArcTopLevelEnv {
    let _span = tracing::info_span!("top_level_env").entered();
    let text: &str = input.text(db);
    let parsed = syntax::parse_query::semantic_parse_file(db, input);
    let cst_root = match &parsed.green {
        Some(green) => syntax::SyntaxNode::new_root(green.as_ref().clone()),
        None => {
            return ArcTopLevelEnv(Arc::new(TopLevelEnvData {
                env: TopLevelEnv::default(),
                pattern_constants: Default::default(),
            }))
        }
    };
    // Honour the workspace's `default Order`, so `$ifdef _DEFAULT_DEC` blocks
    // in the prelude resolve the same way here as in ItemTree lowering.
    let mut symbols = syntax::preprocess::default_symbols();
    if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
        if let Some(order) = hir_def::def_query::workspace_default_order(db, ws) {
            order.apply(&mut symbols);
        }
    }
    let (mut env, pattern_constants) = TopLevelEnv::from_cst_with_symbols(&cst_root, &mut symbols);
    let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
    super::infer::env::apply_callable_signature_metadata(&parsed_file, text, &mut env);
    ArcTopLevelEnv(Arc::new(TopLevelEnvData { env, pattern_constants }))
}

/// Workspace query boundary. Compare semantic data, not a partial hash of
/// signatures: records, mappings, overloads and declaration locations also
/// affect consumers. Derived equality covers new fields automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcWorkspaceContext(pub Arc<crate::infer::WorkspaceContext>);

/// Complete workspace view retained for compatibility and reference tests.
///
/// Inference and type navigation use the incremental per-file contribution
/// index in `infer::dependencies::index`. This legacy query deliberately
/// rebuilds the full view, allowing tests to compare ordered merge semantics.
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

    let result =
        TypeCheckResult { legacy_diagnostics: all_diagnostics, ..TypeCheckResult::default() };
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
    let _solver_budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        Default::default(),
    );

    // The callable's own body, not the file's. Depending on
    // `callable_bodies(file)` here meant editing any function in a file
    // re-inferred every function in it — `an_edit_reinfers_only_what_changed`
    // measures exactly that. rust-analyzer's `infer(def)` depends on
    // `body(def)`; this is that edge.
    let Some(cb) = hir_def::def_query::callable_body(db, id).as_ref() else {
        return ArcInferenceResult(Arc::new(TypeCheckResult::default()));
    };
    let names = crate::infer::dependencies::callable_names(cb);
    let env_data = if hir_def::source_order::program(db, id.file(db)).is_some() {
        hir_def::source_order::for_body(db, id)
            .map(|at| crate::infer::dependencies::ordered::environment(db, at, names.clone()))
    } else {
        None
    };
    let env_data = env_data.unwrap_or_else(|| {
        crate::infer::dependencies::callable_environment(db, id.file(db), names)
    });
    let env = env_data.0.env.clone();
    let pattern_constants = env_data.0.pattern_constants.clone();

    let entry = &*cb.body;
    let text: &str = &cb.text;

    db.unwind_if_revision_cancelled();

    let incomplete_before = crate::solver_budget::SolverBudget::incomplete_count();
    let mut ctx = crate::infer::InferenceContext::new_for_body(
        text,
        entry.body.clone(),
        entry.source_map.clone(),
        env,
        pattern_constants,
    );
    ctx.set_owner(db, id);
    if entry.body.mapping_arms.is_empty() {
        ctx.infer_callable_body_hir(&entry.name, entry);
    } else {
        ctx.infer_mapping_body_hir(&entry.name, entry);
    }
    let mut result = ctx.finish_query();
    db.unwind_if_revision_cancelled();
    // A resource-limited proof can change when the next request has budget.
    // Revalidate only those bodies on the next revision; ordinary bodies retain
    // their fine-grained semantic dependencies and cross-file reuse.
    if crate::solver_budget::SolverBudget::incomplete_count() != incomplete_before {
        db.report_untracked_read();
    }
    result.def_base = entry.def_span.start;
    ArcInferenceResult(Arc::new(result))
}

/// Newtype wrapper for transitive effects (per-file).
#[derive(Clone, Debug)]
pub struct ArcTransitiveEffects(pub Arc<HashMap<String, BTreeSet<hir_def::EffectTag>>>);

// Structural, so salsa can backdate it. See `ArcEffectMap`.
impl PartialEq for ArcTransitiveEffects {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
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
    fn exported_signature_keeps_alias_meaning_without_opening_its_module() {
        use base_db::{
            module_graph::{set_module_graph, ModuleData, ModuleGraph},
            FileId,
        };
        let mut db = TestDb::default();
        let files: Vec<_> = [
            "type word = int",
            "val exported : unit -> word\nfunction exported() = 1",
            "val caller : unit -> int\nfunction caller() = exported()",
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| FileText::new(&db, Arc::from(*s), FileId::from_raw(i as u32)))
        .collect();
        hir_def::def_query::set_workspace_files(&mut db, files.clone());
        set_module_graph(
            &mut db,
            ModuleGraph {
                modules: (0..3)
                    .map(|i| ModuleData {
                        files: vec![FileId::from_raw(i)],
                        requires: if i == 0 { vec![] } else { vec![i as usize - 1] },
                    })
                    .collect(),
                globals: vec![],
            },
        );
        let id = hir_def::def_query::file_def_with_body_ids(&db, files[2])[0];
        let result = &infer(&db, id).0;
        assert!(result.type_mismatches.is_empty(), "{:?}", result.type_mismatches);
        assert!(result.inference_diagnostics().is_empty(), "{:?}", result.inference_diagnostics());
        assert!(declaring_type_file(&db, files[2], "word").is_none());
    }

    #[test]
    fn module_graph_changes_invalidate_only_affected_scopes() {
        use base_db::{
            module_graph::{set_module_graph, ModuleData, ModuleGraph},
            FileId,
        };
        let mut db = TestDb::default();
        let files: Vec<_> = [
            "val leaf : unit -> int\nfunction leaf() = 1",
            "val caller : unit -> int\nfunction caller() = leaf()",
            "val independent : unit -> int\nfunction independent() = 2",
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| FileText::new(&db, Arc::from(*s), FileId::from_raw(i as u32)))
        .collect();
        hir_def::def_query::set_workspace_files(&mut db, files.clone());
        let mut graph = ModuleGraph {
            modules: (0..3)
                .map(|i| ModuleData { files: vec![FileId::from_raw(i)], requires: vec![] })
                .collect(),
            globals: vec![],
        };
        set_module_graph(&mut db, graph.clone());
        let result = |db: &TestDb, i: usize| {
            let id = hir_def::def_query::file_def_with_body_ids(db, files[i])[0];
            infer(db, id).0.clone()
        };
        let blocked = result(&db, 1);
        let unrelated = result(&db, 2);
        assert!(!blocked.inference_diagnostics().is_empty());
        graph.modules[1].requires.push(0);
        set_module_graph(&mut db, graph.clone());
        let allowed = result(&db, 1);
        assert!(
            allowed.inference_diagnostics().is_empty(),
            "{:?}",
            allowed.inference_diagnostics()
        );
        assert!(!Arc::ptr_eq(&blocked, &allowed));
        assert!(Arc::ptr_eq(&unrelated, &result(&db, 2)), "unrelated scope re-inferred");
        graph.modules[1].requires.clear();
        set_module_graph(&mut db, graph);
        assert!(!result(&db, 1).inference_diagnostics().is_empty());
    }

    #[test]
    fn private_declarations_are_visible_across_files_of_one_module() {
        use base_db::{
            module_graph::{set_module_graph, ModuleData, ModuleGraph},
            FileId,
        };
        let mut db = TestDb::default();
        let files: Vec<_> = [
            "private val secret : unit -> int\nfunction secret() = 1",
            "val inside : unit -> int\nfunction inside() = secret()",
            "val outside : unit -> int\nfunction outside() = secret()",
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| FileText::new(&db, Arc::from(*s), FileId::from_raw(i as u32)))
        .collect();
        hir_def::def_query::set_workspace_files(&mut db, files.clone());
        set_module_graph(
            &mut db,
            ModuleGraph {
                modules: vec![
                    ModuleData {
                        files: vec![FileId::from_raw(0), FileId::from_raw(1)],
                        requires: vec![],
                    },
                    ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
                ],
                globals: vec![],
            },
        );
        let result = |i: usize| {
            let id = hir_def::def_query::file_def_with_body_ids(&db, files[i])[0];
            infer(&db, id).0.clone()
        };
        assert!(result(1).inference_diagnostics().is_empty());
        assert!(!result(2).inference_diagnostics().is_empty());
    }

    fn call_type(sources: &[&str], owner: &str, callee: &str) -> crate::ty::Ty {
        let mut db = TestDb::default();
        let files: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, source)| {
                FileText::new(&db, Arc::from(*source), base_db::FileId::from_raw(i as u32))
            })
            .collect();
        hir_def::def_query::set_workspace_files(&mut db, files.clone());
        for file in files {
            for &id in hir_def::def_query::file_def_with_body_ids(&db, file) {
                if id.name(&db).as_str() != owner {
                    continue;
                }
                let result = &infer(&db, id).0;
                let body = &hir_def::def_query::body_with_source_map(&db, id).0 .0;
                for (expr, node) in body.store.exprs.iter() {
                    if let hir_def::Expr::Call { callee: name, .. } = node {
                        if matches!(body.expr(*name), Some(hir_def::Expr::Ident(name)) if name == callee)
                        {
                            let ty = result.expr_ty(expr).unwrap();
                            assert!(!ty.is_error() && !ty.references_error(), "{callee}: {ty:?}");
                            assert!(
                                result.type_mismatches.is_empty(),
                                "{:?}",
                                result.type_mismatches
                            );
                            return ty.clone();
                        }
                    }
                }
            }
        }
        panic!("missing call {owner}/{callee}")
    }

    #[test]
    fn cross_file_calls_preserve_symbolic_numeric_return_bounds() {
        let ty = call_type(&[
            "val unsigned : forall 'n. bits('n) -> range(0, 2 ^ 'n - 1)",
            "val caller : forall 'n. bits('n) -> range(0, 2 ^ 'n - 1)\nfunction caller(x) = unsigned(x)",
        ], "caller", "unsigned");
        assert_eq!(ty, crate::infer::type_from_type_text("range(0, 2 ^ 'n - 1)"));
    }

    #[test]
    fn inline_compound_widths_retain_their_quantified_parameter() {
        let ty = call_type(&[
            "function update forall 'n, 'n in {4, 8}. (size : int('n), value : bits('n * 8)) -> bits('n * 8) = value\nval caller : forall 'n, 'n in {4, 8}. (int('n), bits('n * 8)) -> bits('n * 8)\nfunction caller(n, value) = update(n, value)",
        ], "caller", "update");
        assert_eq!(ty, crate::infer::type_from_type_text("bits('n * 8)"));
    }

    #[test]
    fn call_constraints_reuse_chained_symbolic_assumptions() {
        let ty = call_type(&[
            "type limit : Int = 4096\nval read : forall 'n, 0 < 'n <= limit. int('n) -> bits(8 * 'n)\nval caller : forall 'n, 0 < 'n <= limit. int('n) -> bits(8 * 'n)\nfunction caller(n) = read(n)",
        ], "caller", "read");
        assert_eq!(ty, crate::infer::type_from_type_text("bits(8 * 'n)"));
    }

    #[test]
    fn conditional_singleton_arguments_keep_the_selected_width() {
        let ty = call_type(&[
            "val width : forall 'n, 'n in {4, 8}. int('n) -> bits('n * 8)\nval caller : forall ('b : Bool). bool('b) -> bits((if 'b then 4 else 8) * 8)\nfunction caller(b) = width(if b then 4 else 8)",
        ], "caller", "width");
        assert_eq!(ty, crate::infer::type_from_type_text("bits((if 'b then 4 else 8) * 8)"));
    }

    #[test]
    fn call_constraints_expand_symbolic_configuration_aliases() {
        let ty = call_type(&[
            "type wide : Bool = config base.wide\ntype lanes : Int = if wide then 8 else 4\ntype flen : Int = lanes * 8\ntype word = bits(flen)\nval pack : forall 'n 'm, 'n <= 'm. (implicit('m), bits('n)) -> bits('m)\nfunction caller(value : bits(16)) -> word = pack(value)",
        ], "caller", "pack");
        assert_eq!(ty.display_text(), "bits(flen)");
    }

    #[test]
    fn compound_numeric_results_resolve_function_arguments() {
        let ty = call_type(&[
            "val divide : forall 'n 'm. (int('n), int('m)) -> int(div('n, 'm))\nval caller : forall 'a 'b. (int('a), int('b)) -> int\nfunction caller(a, b) = divide(a, b)",
        ], "caller", "divide");
        assert_eq!(
            ty,
            crate::ty::Ty::app(
                "int",
                vec![crate::ty::TyArg::Nexp(crate::ty::NumericExpr::parse("div('a, 'b)").unwrap())],
                "int(div('a, 'b))"
            )
        );
    }

    #[test]
    fn cross_file_configuration_facts_and_value_guards_share_identity() {
        let ty = call_type(&[
            "type xlen : Int = config base.xlen\nconstraint xlen in {32, 64}\ntype xlen_bytes : Int = if xlen == 32 then 4 else 8\nlet xlen_bytes = sizeof(xlen_bytes)",
            "val read : forall 'n. int('n) -> bits(8 * 'n)\nval extend : forall 'n, 0 < 'n <= xlen. bits('n) -> bits(xlen)\nfunction caller(width : {1, 2, 4, 8}) -> bits(xlen) = { assert(width <= xlen_bytes); let data = read(width); extend(data) }",
        ], "caller", "extend");
        assert_eq!(ty.display_text(), "bits(xlen)");
    }

    #[test]
    fn global_constraint_edits_invalidate_only_dependent_callables() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let config = FileText::new(
            &db,
            Arc::from("type limit : Int = config limits.n\nconstraint limit == 8"),
            base_db::FileId::from_raw(0),
        );
        let caller = FileText::new(&db, Arc::from("val accept : forall 'n, 'n == limit. int('n) -> unit\nfunction caller() -> unit = accept(8)\nfunction unrelated() -> unit = ()"), base_db::FileId::from_raw(1));
        hir_def::def_query::set_workspace_files(&mut db, vec![config, caller]);
        let results = |db: &TestDb| {
            hir_def::def_query::file_def_with_body_ids(db, caller)
                .iter()
                .map(|&id| infer(db, id).0.clone())
                .collect::<Vec<_>>()
        };
        let before = results(&db);
        config
            .set_text(&mut db)
            .to(Arc::from("type limit : Int = config limits.n\nconstraint limit == 16"));
        let after = results(&db);
        assert!(!Arc::ptr_eq(&before[0], &after[0]), "dependent callable stayed cached");
        assert!(Arc::ptr_eq(&before[1], &after[1]), "unrelated callable was invalidated");
        for &id in hir_def::def_query::file_def_with_body_ids(&db, caller) {
            assert_matches_full_environment(&db, id);
        }
    }

    #[test]
    fn bounded_range_arguments_keep_a_rigid_runtime_value() {
        let ty = call_type(&[
            "val rotate : forall 'm 'n, 'm >= 'n >= 0. (bits('m), int('n)) -> bits('m)\nval caller : forall 'm 'n, 'n >= 0 & 'm >= 2 ^ 'n. (bits('m), range(0, 2 ^ 'n - 1)) -> bits('m)\nfunction caller(value, shift) = rotate(value, shift)",
        ], "caller", "rotate");
        assert_eq!(ty, crate::infer::type_from_type_text("bits('m)"));
    }

    #[test]
    fn empty_lists_take_their_element_type_from_context_and_equality() {
        let source = "struct region = { base : int }\ntype regions = list(region)\nfunction direct() -> regions = [||]\nfunction left(xs : regions) -> bool = [||] == xs\nfunction right(xs : regions) -> bool = xs != [||]";
        let mut db = TestDb::default();
        let file = FileText::new(&db, Arc::from(source), base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file]);
        for &id in hir_def::def_query::file_def_with_body_ids(&db, file) {
            let result = &infer(&db, id).0;
            let body = &hir_def::def_query::body_with_source_map(&db, id).0 .0;
            for (expr, _) in
                body.store.exprs.iter().filter(|(_, e)| matches!(e, hir_def::Expr::List(_)))
            {
                let ty = result.expr_ty(expr).unwrap();
                assert_eq!(ty.display_text(), "list(region)", "{}", id.name(&db));
                assert!(!ty.references_error());
            }
        }
    }

    #[test]
    fn exit_checks_and_records_its_unit_operand() {
        for (source, valid) in [
            ("function quit() -> int = exit()", true),
            ("function quit() -> int = exit(42)", false),
        ] {
            let mut db = TestDb::default();
            let file = FileText::new(&db, Arc::from(source), base_db::FileId::from_raw(0));
            hir_def::def_query::set_workspace_files(&mut db, vec![file]);
            let id = hir_def::def_query::file_def_with_body_ids(&db, file)[0];
            let result = &infer(&db, id).0;
            let body = &hir_def::def_query::body_with_source_map(&db, id).0 .0;
            let operand = body
                .store
                .exprs
                .iter()
                .find_map(
                    |(_, expr)| {
                        if let hir_def::Expr::Exit(value) = expr {
                            *value
                        } else {
                            None
                        }
                    },
                )
                .unwrap();
            let ty = result.expr_ty(operand).expect("exit operand was not inferred");
            assert!(!ty.is_error());
            assert_eq!(result.type_mismatches.is_empty(), valid);
        }
    }

    #[test]
    #[cfg(feature = "z3-solver")]
    fn budget_limited_inference_is_retried_after_an_unrelated_edit() {
        use crate::solver_budget::{SolverBudget, SolverLimits};
        use salsa::Setter;
        let mut db = TestDb::default();
        let file=FileText::new(&db,Arc::from("val need : forall 'n, 'n * 'n + 987654321 >= 0. int('n) -> unit\nval caller : forall 'm. int('m) -> unit\nfunction caller(x) = need(x)"),base_db::FileId::from_raw(0));
        let other = FileText::new(
            &db,
            Arc::from("function unrelated() -> int = 1"),
            base_db::FileId::from_raw(1),
        );
        hir_def::def_query::set_workspace_files(&mut db, vec![file, other]);
        let id = hir_def::def_query::file_def_with_body_ids(&db, file)[0];
        let limited = SolverBudget::enter(
            crate::CancellationToken::never(),
            SolverLimits { checks: 0, ..Default::default() },
        );
        let old = infer(&db, id).0.clone();
        assert!(SolverBudget::incomplete_count() > 0, "fixture must exhaust the SMT budget");
        drop(limited);
        other.set_text(&mut db).to(Arc::from("function unrelated() -> int = 2"));
        let id = hir_def::def_query::file_def_with_body_ids(&db, file)[0];
        let new = infer(&db, id).0.clone();
        assert!(
            !Arc::ptr_eq(&old, &new),
            "resource-limited inference must not remain permanently cached"
        );
    }

    /// Keep both results alive so pointer comparisons cannot pass because an
    /// allocator reused a freed address. Exercise a real dependent query.
    #[test]
    fn signature_edits_invalidate_cross_file_inference() {
        use salsa::Setter;
        let cases = [
            (
                "type word = bits(xlen)\ntype xlen : Int = 8",
                "type word = bits(xlen)\ntype xlen : Int = 16",
                "function caller() -> word = 0xff",
            ),
            (
                "type predicate('p : Bool) = bool('p)",
                "type predicate('p : Bool) = bool(not('p))",
                "function caller(x : predicate(true)) -> bool(true) = x",
            ),
            (
                "type width = bits(if true then 8 else 16)",
                "type width = bits(if false then 8 else 16)",
                "function caller() -> width = 0xff",
            ),
            (
                "val callee : unit -> {'n, 'n == 8. bits('n)}",
                "val callee : unit -> {'n, 'n in {8, 16}. bits('n)}",
                "function caller() -> bits(8) = callee()",
            ),
            (
                "val callee : int -> int",
                "val callee : bool -> int",
                "function caller() -> int = callee(1)",
            ),
            (
                "struct R = { field : int }",
                "struct R = { field : bool }",
                "function caller(r : R) -> int = r.field",
            ),
            ("type T = int", "type T = bool", "function caller(x : T) -> T = x"),
            (
                "type combined('n : Int, 'nn : Int) = bits('nn + 'n)",
                "type combined('n : Int, 'nn : Int) = bits('nn + 'n + 1)",
                "function caller() -> combined(4, 8) = 0xabc",
            ),
            (
                "type bounded('m : Int) = {'n, 'n < 'm. bits('n)}",
                "type bounded('m : Int) = {'n, 'n <= 'm. bits('n)}",
                "function caller(x : bounded(8)) -> bounded(8) = x",
            ),
            ("union U = { C : int }", "union U = { C : bool }", "function caller() -> U = C(1)"),
            ("val m : int <-> bool", "val m : string <-> bool", "function caller() -> bool = m(1)"),
            (
                "val a : int -> int\nval b : int -> bool\noverload callee = {a}",
                "val a : int -> int\nval b : int -> bool\noverload callee = {b}",
                "function caller() -> int = callee(1)",
            ),
            (
                "val callee : forall 'n, 'n > 0. int('n) -> int",
                "val callee : forall 'n, 'n > 1. int('n) -> int",
                "function caller() -> int = callee(1)",
            ),
            (
                r#"val callee : int -> int = pure "callee""#,
                r#"val callee : int -> int = impure "callee""#,
                "function caller() -> int = callee(1)",
            ),
            (
                "bitfield R : bits(8) = { field : 7 .. 0 }",
                "bitfield R : bits(8) = { field : 3 .. 0 }",
                "function caller(r : R) -> bits(8) = r.field",
            ),
        ];
        for (before, after, source) in cases {
            let mut db = TestDb::default();
            let dependency = FileText::new(&db, Arc::from(before), base_db::FileId::from_raw(0));
            let caller = FileText::new(
                &db,
                Arc::from(format!("{source}\nfunction independent() -> int = 0\n")),
                base_db::FileId::from_raw(1),
            );
            hir_def::def_query::set_workspace_files(&mut db, vec![dependency, caller]);
            let result = |db: &TestDb| {
                let ids = hir_def::def_query::file_def_with_body_ids(db, caller);
                infer(db, ids[0]).0.clone()
            };
            let independent = |db: &TestDb| {
                let ids = hir_def::def_query::file_def_with_body_ids(db, caller);
                infer(db, ids[1]).0.clone()
            };
            let old_independent = independent(&db);
            let old = result(&db);
            dependency.set_text(&mut db).to(Arc::from(after));
            let new = result(&db);
            assert!(!Arc::ptr_eq(&old, &new), "stale inference after {before:?} -> {after:?}");
            assert!(
                Arc::ptr_eq(&old_independent, &independent(&db)),
                "unrelated inference invalidated by {before:?}"
            );
            let ids = hir_def::def_query::file_def_with_body_ids(&db, caller);
            assert_matches_full_environment(&db, ids[0]);
        }
    }

    #[test]
    fn symbol_insert_delete_rename_and_transitive_edits() {
        use salsa::Setter;
        let cases = [
            ("", "val target : unit -> int", "function caller() -> int = target()"),
            ("val target : unit -> int", "", "function caller() -> int = target()"),
            (
                "val target : unit -> int",
                "val renamed : unit -> int",
                "function caller() -> int = target()",
            ),
            (
                "val member : int -> int\noverload target = {member}",
                "val member : int -> bool\noverload target = {member}",
                "function caller() -> int = target(1)",
            ),
            (
                "type A = int\ntype B = A\nval target : unit -> B",
                "type A = bool\ntype B = A\nval target : unit -> B",
                "function caller() -> int = target()",
            ),
            (
                "struct R = { field : int }\nval target : unit -> R",
                "struct R = { field : bool }\nval target : unit -> R",
                "function caller() -> int = target().field",
            ),
            (
                "enum E = { A, B }",
                "enum E = { A, B, C }",
                "function caller(x : E) -> int = match x { A => 0, B => 1 }",
            ),
            ("register r : int", "register r : bool", "function caller() -> int = r"),
            (
                "val m : int <-> bool",
                "val m : string <-> bool",
                "function caller() -> bool = m_forwards(1)",
            ),
            (
                "struct R = { field : int }",
                "struct R = { field : int }\nstruct S = { field : bool }",
                "function caller() -> unit = { let r = struct { field = 1 }; () }",
            ),
            (
                "bitfield R : bits(8) = { field : 7 .. 0 }",
                "bitfield R : bits(8) = { field : 3 .. 0 }",
                "function caller(r : R) -> R = update_field(r, 0xFF)",
            ),
        ];
        for (before, after, source) in cases {
            // Both same-file and cross-file references must be selective.
            for same_file in [false, true] {
                let mut db = TestDb::default();
                let body = format!("{source}\nfunction independent() -> int = 0\n");
                let dependency = FileText::new(
                    &db,
                    Arc::from(if same_file {
                        format!("{before}\n{body}")
                    } else {
                        before.to_owned()
                    }),
                    base_db::FileId::from_raw(0),
                );
                let caller = if same_file {
                    dependency
                } else {
                    FileText::new(&db, Arc::from(body.as_str()), base_db::FileId::from_raw(1))
                };
                let files = if same_file { vec![dependency] } else { vec![dependency, caller] };
                hir_def::def_query::set_workspace_files(&mut db, files);
                let results = |db: &TestDb| {
                    hir_def::def_query::file_def_with_body_ids(db, caller)
                        .iter()
                        .map(|&id| infer(db, id).0.clone())
                        .collect::<Vec<_>>()
                };
                let old = results(&db);
                for &id in hir_def::def_query::file_def_with_body_ids(&db, caller) {
                    assert_matches_full_environment(&db, id);
                }
                dependency.set_text(&mut db).to(Arc::from(if same_file {
                    format!("{after}\n{body}")
                } else {
                    after.to_owned()
                }));
                let new = results(&db);
                assert!(
                    !Arc::ptr_eq(&old[0], &new[0]),
                    "stale result: {before} -> {after} (local={same_file})"
                );
                assert!(
                    Arc::ptr_eq(&old[1], &new[1]),
                    "unrelated result: {before} -> {after} (local={same_file})"
                );
                for &id in hir_def::def_query::file_def_with_body_ids(&db, caller) {
                    assert_matches_full_environment(&db, id);
                }
            }
        }
    }

    #[test]
    fn workspace_membership_and_changed_body_dependencies() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let caller = FileText::new(
            &db,
            Arc::from("function caller() -> int = f()\nfunction independent() -> int = 0"),
            base_db::FileId::from_raw(0),
        );
        let decls = FileText::new(
            &db,
            Arc::from("val f : unit -> int\nval g : unit -> int"),
            base_db::FileId::from_raw(1),
        );
        hir_def::def_query::set_workspace_files(&mut db, vec![caller]);
        let results = |db: &TestDb| {
            hir_def::def_query::file_def_with_body_ids(db, caller)
                .iter()
                .map(|&id| infer(db, id).0.clone())
                .collect::<Vec<_>>()
        };
        let mut previous = results(&db);
        assert!(!previous[0].inference_diagnostics().is_empty());
        for files in [vec![caller, decls], vec![caller], vec![caller, decls]] {
            hir_def::def_query::set_workspace_files(&mut db, files.clone());
            let next = results(&db);
            assert!(!Arc::ptr_eq(&previous[0], &next[0]));
            assert!(Arc::ptr_eq(&previous[1], &next[1]));
            assert_eq!(next[0].inference_diagnostics().is_empty(), files.len() == 2);
            previous = next;
        }
        caller
            .set_text(&mut db)
            .to(Arc::from("function caller() -> int = g()\nfunction independent() -> int = 0"));
        previous = results(&db);
        decls.set_text(&mut db).to(Arc::from("val f : unit -> bool\nval g : unit -> int"));
        let next = results(&db);
        assert!(
            previous.iter().zip(&next).all(|(a, b)| Arc::ptr_eq(a, b)),
            "old body dependencies were retained"
        );
        decls.set_text(&mut db).to(Arc::from("val f : unit -> bool\nval g : unit -> bool"));
        let next = results(&db);
        assert!(!Arc::ptr_eq(&previous[0], &next[0]));
        assert!(!next[0].type_mismatches.is_empty());
        assert!(Arc::ptr_eq(&previous[1], &next[1]));
    }

    /// Compare against the previous, complete-environment inference path.
    fn assert_matches_full_environment<'db>(
        db: &'db TestDb,
        id: hir_def::def_query::DefWithBodyId<'db>,
    ) {
        let data = top_level_env(db, id.file(db));
        let mut env = data.0.env.clone();
        let mut constants = data.0.pattern_constants.clone();
        if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
            workspace_context(db, ws).0.apply_to(&mut env, &mut constants);
            env.has_workspace_context = true;
            env.report_unknown_calls = true;
        }
        let cb = hir_def::def_query::callable_body(db, id).as_ref().unwrap();
        let entry = &cb.body;

        let mut ctx = crate::infer::InferenceContext::new_for_body(
            &cb.text,
            entry.body.clone(),
            entry.source_map.clone(),
            env,
            constants,
        );
        ctx.set_owner(db, id);
        if entry.body.mapping_arms.is_empty() {
            ctx.infer_callable_body_hir(&entry.name, entry);
        } else {
            ctx.infer_mapping_body_hir(&entry.name, entry);
        }
        let full = ctx.finish_query();
        let projected = &infer(db, id).0;
        assert_eq!(
            projected.expression_types().collect::<HashMap<_, _>>(),
            full.expression_types().collect::<HashMap<_, _>>()
        );
        assert_eq!(
            projected.pattern_types().collect::<HashMap<_, _>>(),
            full.pattern_types().collect::<HashMap<_, _>>()
        );
        assert_eq!(projected.type_mismatches, full.type_mismatches);
        assert_eq!(projected.inference_diagnostics(), full.inference_diagnostics());
        // Extra unrelated global assumptions may exist in the full environment;
        // the emitted goals, origins and instantiated bindings must still agree.
        let goals = |result: &crate::infer::InferenceResult| {
            result
                .unsolved_constraints
                .iter()
                .map(|ob| {
                    let mut types: Vec<_> = ob
                        .substitution
                        .types
                        .entries
                        .iter()
                        .map(|(k, v)| (k.clone(), v.display_text()))
                        .collect();
                    let mut values = ob.substitution.values.entries.to_vec();
                    types.sort();
                    values.sort();
                    format!(
                        "{:?} {:?} {:?} {} {:?} {types:?} {values:?}",
                        ob.goal, ob.origin, ob.cause, ob.universe, ob.rigid_variables
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(goals(projected), goals(&full));
    }

    #[test]
    fn callable_environments_share_workspace_indexes() {
        let mut db = TestDb::default();
        let file = FileText::new(
            &db,
            Arc::from("val f : unit -> int\nfunction f() = 0\n"),
            base_db::FileId::from_raw(0),
        );
        hir_def::def_query::set_workspace_files(&mut db, vec![file]);
        let workspace = workspace_context(&db, base_db::WorkspaceFiles::try_get(&db).unwrap());
        let mut a = TopLevelEnv::default();
        let mut b = TopLevelEnv::default();
        workspace.0.apply_to(&mut a, &mut HashSet::new());
        workspace.0.apply_to(&mut b, &mut HashSet::new());
        assert!(Arc::ptr_eq(&a.cross_file_function_names, &b.cross_file_function_names));
        assert!(Arc::ptr_eq(
            a.cross_file_schemes.as_ref().unwrap(),
            b.cross_file_schemes.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(&a.cross_file_function_arity, &b.cross_file_function_arity));
        assert!(Arc::ptr_eq(&a.symbol_index, &b.symbol_index));
        assert!(a.cross_file_function_arity["f"].contains(&(0, 0)));
    }

    #[test]
    fn body_edit_preserves_cross_file_inference() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let dependency = FileText::new(
            &db,
            Arc::from("function callee() -> int = 1\n"),
            base_db::FileId::from_raw(0),
        );
        let caller = FileText::new(
            &db,
            Arc::from("function caller() -> int = callee()\n"),
            base_db::FileId::from_raw(1),
        );
        hir_def::def_query::set_workspace_files(&mut db, vec![dependency, caller]);
        let result = |db: &TestDb| {
            let ids = hir_def::def_query::file_def_with_body_ids(db, caller);
            infer(db, ids[0]).0.clone()
        };
        let old = result(&db);
        dependency.set_text(&mut db).to(Arc::from("function callee() -> int = 12345\n"));
        assert!(Arc::ptr_eq(&old, &result(&db)), "a body-only edit invalidated its caller");
    }

    #[test]
    fn type_patterns_expand_synonyms_and_check_numeric_solutions() {
        for (alias, actual, pattern, valid) in [
            ("type bytes('n : Int), 'n >= 0 = bits(8 * 'n)", "bits(32)", "bytes('n)", true),
            ("type bytes('n : Int), 'n >= 0 = bits(8 * 'n)", "bits(33)", "bytes('n)", false),
            ("type v8('a : Type) = vector(8, 'a)", "vector(4, int)", "v8(_)", false),
            ("type dup('a : Type) = ('a, 'a)", "(int(1), int(2))", "dup(int('n))", false),
            ("", "bits(4)", "bits(int('n))", false),
            ("", "bool", "bool('b)", true),
            ("", "string", "'n", false),
            ("", "bits(4)", "bits(4)", false),
            (
                "type sw('n : Int, 'm : Int) = (bits('n), bits('m))",
                "(bits(1), bits(2))",
                "sw('m, 'n)",
                true,
            ),
        ] {
            let db = TestDb::default();
            let source = format!("type bits('n) = bitvector('n)\n{alias}\nfunction f(x : {actual}) -> unit = match x {{ y as {pattern} => () }}\n");
            let file = FileText::new(&db, Arc::from(source.as_str()), base_db::FileId::from_raw(0));
            let ids = hir_def::def_query::file_def_with_body_ids(&db, file);
            let result = infer(&db, ids[0]);
            assert_eq!(
                result.0.type_mismatches.is_empty(),
                valid,
                "{source}\n{:?}",
                result.0.type_mismatches
            );
        }
    }

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

    /// Cross-file symbols are visible to inference.
    #[test]
    fn cross_file_include_symbols_visible() {
        let mut db = TestDb::default();
        let b: Arc<str> = Arc::from("val helper : int -> int\nfunction helper(x) = x + 1\n");
        let a: Arc<str> = Arc::from("val main : unit -> int\nfunction main() = helper(42)\n");
        let file_b = FileText::new(&db, b, base_db::FileId::from_raw(1));
        let file_a = FileText::new(&db, a, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_a, file_b]);

        let messages: Vec<String> = hir_def::def_query::file_def_with_body_ids(&db, file_a)
            .iter()
            .flat_map(|&id| {
                infer(&db, id)
                    .0
                    .inference_diagnostics()
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            !messages.iter().any(|m| m.contains("helper")),
            "helper should resolve through the workspace, got: {messages:?}"
        );
    }

    /// A `mapping clause` whose `val` lives in another file still gets the
    /// mapping's type. `lookup_mappings` only ever looked in the current
    /// file, so every `encdec` and `assembly` clause in sail-riscv — the
    /// shape below — bound its pattern variables to nothing.
    #[test]
    fn cross_file_mapping_signature_visible() {
        let mut db = TestDb::default();
        let lib: Arc<str> = Arc::from(
            "default Order dec
             type regidx = bits(5)
             scattered union ast
             val encdec : ast <-> bits(32)
             scattered mapping encdec
",
        );
        let main: Arc<str> = Arc::from(
            "union clause ast = ZICBOZ : regidx
             mapping clause encdec = ZICBOZ(rs1)
               <-> 0b000000000100 @ rs1 @ 0b010 @ 0b00000 @ 0b0001111
",
        );
        let file_lib = FileText::new(&db, lib, base_db::FileId::from_raw(1));
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main, file_lib]);

        let mut env = top_level_env(&db, file_main).0.env.clone();
        assert!(
            !env.mappings.contains_key("encdec"),
            "the mapping is declared in the other file, so it is not local"
        );
        let mut pattern_constants = Default::default();
        let ws = base_db::WorkspaceFiles::try_get(&db).expect("workspace files");
        workspace_context(&db, ws).0.apply_to(&mut env, &mut pattern_constants);
        assert!(
            !env.lookup_mappings("encdec").is_empty(),
            "the cross-file signature should still be found"
        );
    }

    /// A `union clause` in another file carries its payload type, so a
    /// constructor pattern over it binds a real type rather than `error()`.
    #[test]
    fn cross_file_union_clause_payload_visible() {
        let mut db = TestDb::default();
        let lib: Arc<str> = Arc::from(
            "default Order dec
scattered union ast
union clause ast = ZICBOZ : bits(5)
",
        );
        let main: Arc<str> = Arc::from(
            "val f : ast -> unit
function f(x) = ()
",
        );
        let file_lib = FileText::new(&db, lib, base_db::FileId::from_raw(1));
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main, file_lib]);

        let mut env = top_level_env(&db, file_main).0.env.clone();
        let mut pattern_constants = Default::default();
        let ws = base_db::WorkspaceFiles::try_get(&db).expect("workspace files");
        workspace_context(&db, ws).0.apply_to(&mut env, &mut pattern_constants);
        let schemes = env.lookup_functions("ZICBOZ");
        let scheme = schemes.first().expect("the cross-file constructor should be found");
        assert_eq!(
            scheme.params.first().map(|p| p.display_text()),
            Some("bits(5)".to_string()),
            "the payload type should come with it"
        );
    }

    /// An identifier a mapping pattern cannot type on its own takes the type
    /// the other side of the clause gave it.
    ///
    /// A vector-concat pattern has no way to recover an operand's width —
    /// `0b001100 @ vm @ vs2 @ ..` is two unknowns against one total — so the
    /// backwards scope bound `vm` at `error()` and the constructor argument
    /// on the left stayed untyped. Upstream binds each side independently
    /// and then checks it with the other side's environment as fallback
    /// (`check_mpexp right_env env left_mpexp`, type_check.ml:4682). 288 of
    /// sail-riscv's untyped expressions were this one case.
    #[test]
    fn a_mapping_pattern_takes_a_binding_type_from_the_other_side() {
        let mut db = TestDb::default();
        let main: Arc<str> = Arc::from(
            "default Order dec
union instruction = { VCLMUL_VV : (bits(1), bits(5)) }
val encdec : instruction <-> bits(32)
mapping encdec = VCLMUL_VV (vm, vs2)
  <-> 0b001100 @ vm @ vs2 @ 0b111111111111111
",
        );
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main]);

        let found = ident_types(&db, file_main, "vm");
        assert!(!found.is_empty(), "expected to find `vm` in the arena");
        assert!(
            found.iter().all(|t| t == "bits(1)"),
            "every `vm` should carry the constructor's payload type, got {found:?}"
        );
    }

    /// Collect the inferred types of every `Expr::Ident` named `name`.
    fn ident_types(db: &TestDb, file: FileText, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        for &id in hir_def::def_query::file_def_with_body_ids(db, file).iter() {
            let result = infer(db, id).0.clone();
            let body_and_map = hir_def::def_query::body_with_source_map(db, id);
            let body = &body_and_map.0 .0;
            for (expr_id, expr) in body.store.exprs.iter() {
                if matches!(expr, hir_def::Expr::Ident(n) if n == name) {
                    out.push(
                        result
                            .expr_ty(expr_id)
                            .map(|t| t.display_text())
                            .unwrap_or_else(|| "<none>".to_string()),
                    );
                }
            }
        }
        out
    }

    /// Assigning to a register does not shadow it for the rest of the body.
    ///
    /// `Expr::Assign` defines an unknown bare-identifier target as a local so
    /// that later reads do not report `UnresolvedIdent`. It did that without
    /// asking whether the name was already known, and locals are consulted
    /// first — so one `mstatus = ..` anywhere in a function erased the
    /// register's type from every read of it in that function.
    #[test]
    fn assigning_to_a_register_keeps_its_type() {
        let mut db = TestDb::default();
        let main: Arc<str> = Arc::from(
            "default Order dec
register mstatus : bits(64)
val f : bits(64) -> bits(64)
function f(v) = {
  mstatus = v;
  mstatus
}
",
        );
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main]);

        let found = ident_types(&db, file_main, "mstatus");
        assert!(!found.is_empty(), "expected to find `mstatus` in the arena");
        assert!(
            found.iter().all(|t| t == "bits(64)"),
            "the register keeps its type on both sides, got {found:?}"
        );
    }

    /// `F_or_X_D(rd) = v` types its callee.
    ///
    /// Upstream desugars `LE_app(f, xs) = exp` to `E_app(f, xs @ [exp])`
    /// (initial_check.ml). Resolving the whole call that way reports valid
    /// code, so only the part that cannot — reading the setter's signature
    /// off the arity the desugaring implies — is done here. Without it an
    /// overload name inferred as a bare identifier, which is not a value,
    /// and came out `error()`.
    #[test]
    fn an_assignment_to_a_call_types_the_setter() {
        let mut db = TestDb::default();
        let main: Arc<str> = Arc::from(
            "default Order dec
val rF_or_X_D : bits(5) -> bits(64)
val wF_or_X_D : (bits(5), bits(64)) -> unit
overload F_or_X_D = { rF_or_X_D, wF_or_X_D }
val f : (bits(5), bits(64)) -> unit
function f(rd, v) = {
  F_or_X_D(rd) = v
}
",
        );
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main]);

        let found = ident_types(&db, file_main, "F_or_X_D");
        assert!(!found.is_empty(), "expected to find `F_or_X_D` in the arena");
        assert!(
            found.iter().all(|t| t.contains("bits(5)") && t.contains("bits(64)")),
            "the callee should carry the two-parameter setter signature, got {found:?}"
        );
    }

    /// A `function` with no `val` still has a signature, and it has to
    /// survive the trip across files. sail-riscv writes `overload X =
    /// {rX_bits, wX_bits, rX, wX}` where every member is declared that way.
    #[test]
    fn cross_file_bare_function_signature_visible() {
        let mut db = TestDb::default();
        let lib: Arc<str> = Arc::from(
            "default Order dec
             function rX_bits(i : bits(5)) -> bits(64) = sail_zeros(64)
             overload X = {rX_bits}
",
        );
        let main: Arc<str> = Arc::from(
            "val f : unit -> unit
function f() = ()
",
        );
        let file_lib = FileText::new(&db, lib, base_db::FileId::from_raw(1));
        let file_main = FileText::new(&db, main, base_db::FileId::from_raw(0));
        hir_def::def_query::set_workspace_files(&mut db, vec![file_main, file_lib]);

        let mut env = top_level_env(&db, file_main).0.env.clone();
        let mut pattern_constants = Default::default();
        let ws = base_db::WorkspaceFiles::try_get(&db).expect("workspace files");
        workspace_context(&db, ws).0.apply_to(&mut env, &mut pattern_constants);
        let schemes = env.lookup_functions("X");
        assert_eq!(schemes.len(), 1, "the overload should resolve to its one member");
        assert_eq!(schemes[0].ret.display_text(), "bits(64)");
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
