//! Salsa tracked queries for definition-layer data.
//!
//! Builds on `syntax::parse_query::parse_file` to derive:
//! - `file_item_tree` — per-file ItemTree (signature-hash cache key)
//! - `callable_bodies` — per-file CallableBodies arena
//!

use std::sync::Arc;

use base_db::FileText;
use syntax::parse_query::{semantic_parse_file as parse_file, ParsedFileData};

use crate::bodies::CallableBodies;
use crate::callgraph::CallGraph;
use crate::item_tree::ItemTree;
use crate::nameres::DefMap;

/// `Arc<CallableBodies>` with pointer-based Eq/Hash for salsa.
#[derive(Clone, Debug)]
pub struct ArcCallableBodies(pub Arc<CallableBodies>);

impl PartialEq for ArcCallableBodies {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcCallableBodies {}
impl std::hash::Hash for ArcCallableBodies {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

// Include queries moved to hir-expand — re-exported for backward compatibility.
pub use hir_expand::query::{include_paths, typed_include_paths, ArcIncludeGraph, IncludePath};

/// The `default Order dec | inc` declared by a single file, if any.
#[salsa::tracked]
pub fn file_default_order(
    db: &dyn salsa::Database,
    input: FileText,
) -> Option<syntax::preprocess::DefaultOrder> {
    let parsed: &ParsedFileData = parse_file(db, input);
    let green = parsed.green.as_ref()?;
    let root = syntax::SyntaxNode::new_root(green.as_ref().clone());
    syntax::preprocess::default_order_of_file(&root)
}

/// The program-wide `default Order`, taken from whichever workspace file
/// declares it.
///
/// Upstream reaches the same answer positionally: `default Order dec` is
/// preprocessed before the `$include <prelude.sail>` that follows it, and the
/// symbol set is global and mutable for the rest of the run
/// (preprocess.ml:179-303). Sail permits only one default order per program,
/// so resolving it workspace-wide is equivalent while staying compatible with
/// per-file salsa queries.
#[salsa::tracked]
pub fn workspace_default_order(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
) -> Option<syntax::preprocess::DefaultOrder> {
    ws.file_texts(db).iter().find_map(|&ft| file_default_order(db, ft))
}

/// Preprocessor symbols in effect for the whole workspace.
fn workspace_symbols(db: &dyn salsa::Database) -> std::collections::HashSet<String> {
    let mut symbols = syntax::preprocess::default_symbols();
    if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
        if let Some(order) = workspace_default_order(db, ws) {
            order.apply(&mut symbols);
        }
    }
    symbols
}

/// Build the per-file ItemTree from a parsed file.
#[salsa::tracked(returns(ref))]
pub fn file_item_tree(db: &dyn salsa::Database, input: FileText) -> Option<Arc<ItemTree>> {
    let parsed: &ParsedFileData = parse_file(db, input);
    let green = parsed.green.as_ref()?;
    let root = syntax::SyntaxNode::new_root(green.as_ref().clone());
    let mut symbols = workspace_symbols(db);
    Some(Arc::new(ItemTree::build_from_cst_full(&root, &mut symbols)))
}

/// Build per-callable Body arenas from a parsed file.
#[salsa::tracked(returns(ref))]
pub fn callable_bodies(db: &dyn salsa::Database, input: FileText) -> Option<ArcCallableBodies> {
    let parsed: &ParsedFileData = parse_file(db, input);
    let green = parsed.green.as_ref()?;
    let root = syntax::SyntaxNode::new_root(green.as_ref().clone());
    Some(ArcCallableBodies(Arc::new(CallableBodies::from_cst(&root))))
}

/// Newtype wrapper for `Arc<CallGraph>` with pointer-based Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcCallGraph(pub Arc<CallGraph>);

// Structural: a callgraph is maps of names and spans, which compare
// directly, and comparing them is what lets salsa backdate. `Arc::ptr_eq`
// never reports equal, because a re-executed query always allocates.
impl PartialEq for ArcCallGraph {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ArcCallGraph {}
impl std::hash::Hash for ArcCallGraph {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.site_count().hash(state);
    }
}

/// Newtype for `Arc<DefMap>` with pointer-based Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcDefMap(pub Arc<DefMap>);

// Structural: a def map is one `ItemScope` plus a row per definition,
// both of which compare directly. See `ArcCallGraph`.
impl PartialEq for ArcDefMap {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ArcDefMap {}
impl std::hash::Hash for ArcDefMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
    }
}

/// Build the per-file DefMap (name resolution index).
#[salsa::tracked(returns(ref))]
pub fn file_def_map(db: &dyn salsa::Database, input: FileText) -> Option<ArcDefMap> {
    let item_tree = file_item_tree(db, input).as_ref()?;
    Some(ArcDefMap(Arc::new(DefMap::build(item_tree))))
}

/// Salsa tracked function: build the per-file CallGraph from callable bodies.
#[salsa::tracked(returns(ref))]
pub fn callgraph(db: &dyn salsa::Database, input: FileText) -> Option<ArcCallGraph> {
    let bodies = callable_bodies(db, input);
    bodies.as_ref().map(|b| ArcCallGraph(Arc::new(CallGraph::from_callable_bodies(&b.0))))
}

/// Newtype for `Arc<HashMap<String, BTreeSet<EffectTag>>>` with pointer Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcEffectMap(
    pub Arc<std::collections::HashMap<String, std::collections::BTreeSet<crate::EffectTag>>>,
);

// A `HashMap<String, BTreeSet<EffectTag>>` compares structurally already,
// and comparing it is what lets salsa backdate: a re-executed query always
// allocates a fresh `Arc`, so `Arc::ptr_eq` reports "changed" every time
// and every dependent re-runs.
impl PartialEq for ArcEffectMap {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ArcEffectMap {}
impl std::hash::Hash for ArcEffectMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
    }
}

/// Per-file transitive effect computation via worklist fixed-point.
#[salsa::tracked(returns(ref))]
pub fn transitive_effects(db: &dyn salsa::Database, input: FileText) -> ArcEffectMap {
    use crate::EffectTag;
    use std::collections::{BTreeSet, HashMap};

    let bodies = callable_bodies(db, input);
    let callgraph = callgraph(db, input);

    let mut effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::new();

    // Seed with direct effects from each callable body
    if let Some(bodies) = bodies.as_ref() {
        for entry in bodies.0.entries() {
            effects.insert(entry.name.clone(), entry.effects.clone());
        }
    }

    // Fixed-point propagation: union callee effects into callers
    if let Some(cg) = callgraph.as_ref() {
        let max_iterations = effects.len() + 1;
        for _ in 0..max_iterations {
            let mut changed = false;
            let snapshot: Vec<(String, BTreeSet<EffectTag>)> =
                effects.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

            for (name, current) in &snapshot {
                let callee_effects: BTreeSet<EffectTag> =
                    cg.0.callees_of(name)
                        .flat_map(|callee| {
                            snapshot
                                .iter()
                                .find(|(n, _)| n == callee)
                                .map(|(_, e)| e.iter().copied())
                                .into_iter()
                                .flatten()
                        })
                        .collect();

                let merged: BTreeSet<EffectTag> = current.union(&callee_effects).copied().collect();
                if merged.len() > current.len() {
                    effects.insert(name.clone(), merged);
                    changed = true;
                }
            }

            if !changed {
                break;
            }
        }
    }

    ArcEffectMap(Arc::new(effects))
}

use crate::body::{Body, BodySourceMap};

/// One callable's `Body` and `BodySourceMap`, compared by the fingerprint
/// of the callable it was lowered from — the same one `callable_body` uses,
/// and for the same reason: `Arc::ptr_eq` can never report equal, so salsa
/// could never backdate this and every dependent re-ran on any edit.
#[derive(Clone, Debug)]
pub struct ArcBodyWithSourceMap(pub Arc<(Body, BodySourceMap)>, u64);

impl PartialEq for ArcBodyWithSourceMap {
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1
    }
}
impl Eq for ArcBodyWithSourceMap {}
impl std::hash::Hash for ArcBodyWithSourceMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.1.hash(state);
    }
}

/// One callable's lowered body, keyed by the callable rather than the file.
///
/// `infer` used to read `callable_bodies(file)` — a whole-file query — and
/// scan it for its own entry, so editing any function in a file re-inferred
/// every function in it. rust-analyzer's `infer(def)` depends on
/// `body(def)`; this is that dependency.
///
/// Its equality is a fingerprint of the callable's own source text and
/// where it starts, which is exactly what its lowering is a function of.
/// Two consequences worth knowing: an edit that does not touch a callable
/// leaves its fingerprint alone, so salsa backdates this query and the
/// callable's `infer` is reused; and an edit that shifts a later callable's
/// start *does* change it, because the spans recorded in the body moved.
/// rust-analyzer avoids the second by keeping spans in a separate
/// `body_with_source_map` query and leaving `body` position-free — the
/// same split would work here, and `Body` bundling its source map is what
/// stands in the way.
#[derive(Clone, Debug)]
pub struct ArcCallableBody {
    pub body: Arc<crate::bodies::CallableBody>,
    /// The file's text, carried here rather than read from `FileText`
    /// directly. Inference needs it to slice spans, and reading it from the
    /// input would make every `infer` in a file depend on the whole file —
    /// which is the dependency this query exists to break. Only spans inside
    /// the callable are ever sliced, and both their content and the
    /// callable's start are in the fingerprint, so a result that compares
    /// equal yields the same characters.
    pub text: Arc<str>,
    fingerprint: u64,
}

impl ArcCallableBody {
    /// The fingerprint of the callable's own source, for queries that
    /// derive from this one and want to backdate with it.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

impl PartialEq for ArcCallableBody {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}
impl Eq for ArcCallableBody {}
impl std::hash::Hash for ArcCallableBody {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.fingerprint.hash(state);
    }
}

#[salsa::tracked(returns(ref), lru = 512)]
pub fn callable_body<'db>(
    db: &'db dyn salsa::Database,
    id: DefWithBodyId<'db>,
) -> Option<ArcCallableBody> {
    use std::hash::{Hash, Hasher};

    let file = id.file(db);
    let bodies = callable_bodies(db, file).as_ref()?;
    let target_name = id.name(db);
    let target_clause = id.clause_index(db);

    let mut clause_idx = 0u32;
    for entry in bodies.0.entries() {
        if entry.name != target_name {
            continue;
        }
        if clause_idx != target_clause {
            clause_idx += 1;
            continue;
        }
        let text: &Arc<str> = file.text(db);
        let mut h = rustc_hash::FxHasher::default();
        // The callable's own characters, and nothing about where they are.
        // Hashing the start too meant a keystroke anywhere above re-inferred
        // everything below it, which is what
        // `typing_a_character_does_not_reinfer_the_file` measures. The
        // positions this value still carries are the ones it was lowered
        // with; `body_with_source_map` rebases them.
        text.get(entry.def_span.start..entry.def_span.end).unwrap_or("").hash(&mut h);
        return Some(ArcCallableBody {
            body: Arc::new(entry.clone()),
            text: Arc::clone(text),
            fingerprint: h.finish(),
        });
    }
    None
}

/// Where this callable starts in the file as it is now.
///
/// Read from `callable_bodies`, which re-runs whenever the file changes —
/// unlike `callable_body`, whose whole point is that it does not.
fn current_def_start<'db>(db: &'db dyn salsa::Database, id: DefWithBodyId<'db>) -> Option<usize> {
    let bodies = callable_bodies(db, id.file(db)).as_ref()?;
    let target_name = id.name(db);
    let mut clause_idx = 0u32;
    for entry in bodies.0.entries() {
        if entry.name != target_name {
            continue;
        }
        if clause_idx != id.clause_index(db) {
            clause_idx += 1;
            continue;
        }
        return Some(entry.def_span.start);
    }
    None
}

/// Per-callable Body + BodySourceMap query.
#[salsa::tracked(returns(ref), lru = 512)]
pub fn body_with_source_map<'db>(
    db: &'db dyn salsa::Database,
    id: DefWithBodyId<'db>,
) -> ArcBodyWithSourceMap {
    let file = id.file(db);
    let file_id = file.file_id(db);

    // Through `callable_body`, so this inherits its fingerprint: an edit
    // that leaves a callable alone leaves that query's value equal, and
    // salsa backdates it instead of re-inferring.
    //
    // Positions are the one thing that edit *did* change. `callable_body`
    // holds the ones the body was lowered with, so this query — which
    // re-runs whenever the file does, and only clones — moves them to where
    // the callable sits now. Everything that shows a user a position comes
    // through here.
    if let Some(cb) = callable_body(db, id).as_ref() {
        let body = (*cb.body.body).clone();
        let mut source_map = (*cb.body.source_map).clone();
        // Populate file_id so BodySourceMap can produce InFile<Span> via
        // expr_syntax_in_file / pat_syntax_in_file.
        if source_map.file_id.is_none() {
            source_map.file_id = Some(file_id);
        }
        // Reading this is what makes this query re-run. Without it every
        // input verifies unchanged after an edit above the callable and the
        // memo is returned — with the positions it had before the edit.
        // `a_reused_body_still_reports_where_it_is_now` fails on exactly
        // that.
        source_map.def_base = current_def_start(db, id).unwrap_or(cb.body.def_span.start);
        return ArcBodyWithSourceMap(Arc::new((body, source_map)), cb.fingerprint());
    }

    // No such callable — an empty body, and a fingerprint that says so.
    let fallback_map = BodySourceMap { file_id: Some(file_id), ..Default::default() };
    ArcBodyWithSourceMap(Arc::new((Body::empty(), fallback_map)), 0)
}

/// Salsa-interned identifier for a callable with a body.
#[salsa::interned]
#[derive(Debug)]
pub struct DefWithBodyId<'db> {
    pub file: FileText,
    pub name: String,
    pub clause_index: u32,
}

/// Build all DefWithBodyIds for a file from its CallableBodies.
#[salsa::tracked(returns(ref))]
pub fn file_def_with_body_ids<'db>(
    db: &'db dyn salsa::Database,
    input: FileText,
) -> Vec<DefWithBodyId<'db>> {
    let bodies = callable_bodies(db, input);
    let mut ids = Vec::new();
    if let Some(bodies) = bodies.as_ref() {
        let mut name_counts: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for entry in bodies.0.entries() {
            let idx = name_counts.entry(&entry.name).or_insert(0);
            ids.push(DefWithBodyId::new(db, input, entry.name.clone(), *idx));
            *idx += 1;
        }
    }
    ids
}

/// Register the workspace's file set, and the operator fixities declared
/// anywhere in it, as salsa inputs.
///
/// The two belong together: `parse_file` reads `WorkspaceFixities`, so a
/// workspace registered without it parses every user-defined operator at the
/// default level. Call this instead of setting `WorkspaceFiles` directly.
///
/// Fixities are collected from a first, fixity-free parse. That is sound
/// because an `infix` declaration is a top-level item whose own parse does
/// not depend on expression precedence.
pub fn set_workspace_files(db: &mut dyn salsa::Database, file_texts: Vec<FileText>) {
    // Create the fixity input *before* anything parses. `parse_file` only
    // records a salsa dependency on an input that already exists, so a table
    // created afterwards would never invalidate the fixity-free parses that
    // collecting the declarations itself triggers.
    if base_db::WorkspaceFixities::try_get(db).is_none() {
        base_db::WorkspaceFixities::new(db, 0, std::collections::HashMap::new());
    }
    match base_db::WorkspaceFiles::try_get(db) {
        Some(existing) if *existing.file_texts(db) == file_texts => {}
        Some(existing) => {
            use salsa::Setter;
            existing.set_file_texts(db).to(file_texts.clone());
        }
        None => {
            base_db::WorkspaceFiles::new(db, file_texts.clone());
        }
    }
    update_workspace_fixities(db, file_texts);
}

/// Merge fixity declarations from all files into the `WorkspaceFixities` input.
pub fn update_workspace_fixities(
    db: &mut dyn salsa::Database,
    file_texts: impl IntoIterator<Item = FileText>,
) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut merged = std::collections::HashMap::new();

    for ft in file_texts {
        if let Some(tree) = file_item_tree(db, ft).as_ref() {
            let ctx = tree.build_fixity_context();
            merged.extend(ctx);
        }
    }

    let mut hasher = DefaultHasher::new();
    let mut keys: Vec<&String> = merged.keys().collect();
    keys.sort();
    for k in keys {
        k.hash(&mut hasher);
        merged[k].hash(&mut hasher);
    }
    let fingerprint = hasher.finish();

    // Only update salsa input if fixities actually changed.
    if let Some(existing) = base_db::WorkspaceFixities::try_get(db) {
        if existing.fingerprint(db) == fingerprint {
            return; // No change
        }
        use salsa::Setter;
        existing.set_fingerprint(db).to(fingerprint);
        existing.set_fixities(db).to(merged);
    } else {
        base_db::WorkspaceFixities::new(db, fingerprint, merged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::FileId;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[test]
    fn item_tree_for_function() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("val foo : int -> int\nfunction foo(x) = x + 1\n"),
            FileId::from_raw(0),
        );

        let tree = file_item_tree(&db, input);
        assert!(tree.is_some(), "should build item tree");
        let tree = tree.as_ref().unwrap();
        assert!(!tree.is_empty(), "should have entries");
    }

    #[test]
    fn callable_bodies_for_function() {
        let db = TestDb::default();
        let input = FileText::new(
            &db,
            Arc::from("function bar(x : int) -> int = x * 2\n"),
            FileId::from_raw(0),
        );

        let bodies = callable_bodies(&db, input);
        assert!(bodies.is_some(), "should build callable bodies");
        let bodies = bodies.as_ref().unwrap();
        assert!(!bodies.0.is_empty(), "should have at least one body");
    }

    #[test]
    fn item_tree_memoized() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val x : int\n"), FileId::from_raw(0));

        let r1 = file_item_tree(&db, input);
        let r2 = file_item_tree(&db, input);
        // Same Arc pointer = memoized
        match (r1.as_ref(), r2.as_ref()) {
            (Some(a), Some(b)) => assert!(Arc::ptr_eq(a, b)),
            _ => panic!("both should be Some"),
        }
    }

    #[test]
    fn item_tree_invalidates_on_signature_change() {
        use salsa::Setter;
        let mut db = TestDb::default();
        let input = FileText::new(&db, Arc::from("val foo : int -> int\n"), FileId::from_raw(0));

        let r1 = file_item_tree(&db, input).clone();

        // Change the type signature
        input.set_text(&mut db).to(Arc::from("val foo : int -> bool\n"));

        let r2 = file_item_tree(&db, input).clone();

        // ItemTree should be different (different signature hash)
        match (r1.as_ref(), r2.as_ref()) {
            (Some(a), Some(b)) => assert_ne!(a.signature_hash, b.signature_hash),
            _ => panic!("both should be Some"),
        }
    }
}
