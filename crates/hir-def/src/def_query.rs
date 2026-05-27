//! Salsa tracked queries for definition-layer data.
//!
//! Builds on `syntax::parse_query::parse_file` to derive:
//! - `file_item_tree` — per-file ItemTree (signature-hash cache key)
//! - `callable_bodies` — per-file CallableBodies arena
//!

use std::sync::Arc;

use base_db::FileText;
use syntax::parse_query::{parse_file, ParsedFileData};

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
pub use hir_expand::query::{
    include_paths, typed_include_paths, ArcIncludeGraph, IncludePath, WorkspaceIncludeGraph,
};

/// Build the per-file ItemTree from a parsed file.
#[salsa::tracked(returns(ref))]
pub fn file_item_tree(db: &dyn salsa::Database, input: FileText) -> Option<Arc<ItemTree>> {
    let parsed: &ParsedFileData = parse_file(db, input);
    let green = parsed.green.as_ref()?;
    let root = syntax::SyntaxNode::new_root(green.as_ref().clone());
    let mut symbols = syntax::preprocess::default_symbols();
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

impl PartialEq for ArcCallGraph {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcCallGraph {}
impl std::hash::Hash for ArcCallGraph {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

/// Newtype for `Arc<DefMap>` with pointer-based Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcDefMap(pub Arc<DefMap>);

impl PartialEq for ArcDefMap {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcDefMap {}
impl std::hash::Hash for ArcDefMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

/// Build the per-file DefMap (name resolution index).
#[salsa::tracked(returns(ref))]
pub fn crate_def_map(db: &dyn salsa::Database, input: FileText) -> Option<ArcDefMap> {
    let item_tree = file_item_tree(db, input).as_ref()?;
    Some(ArcDefMap(Arc::new(DefMap::build(&item_tree))))
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

impl PartialEq for ArcEffectMap {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcEffectMap {}
impl std::hash::Hash for ArcEffectMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
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

/// Newtype for `Arc<(Body, BodySourceMap)>` with pointer-based Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcBodyWithSourceMap(pub Arc<(Body, BodySourceMap)>);

impl PartialEq for ArcBodyWithSourceMap {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcBodyWithSourceMap {}
impl std::hash::Hash for ArcBodyWithSourceMap {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

/// Per-callable Body + BodySourceMap query.
#[salsa::tracked(returns(ref), lru = 512)]
pub fn body_with_source_map<'db>(
    db: &'db dyn salsa::Database,
    id: DefWithBodyId<'db>,
) -> ArcBodyWithSourceMap {
    let file = id.file(db);
    let bodies = callable_bodies(db, file);

    let target_name = id.name(db);
    let target_clause = id.clause_index(db);

    let file_id = file.file_id(db);

    if let Some(bodies) = bodies.as_ref() {
        let mut clause_idx = 0u32;
        for entry in bodies.0.entries() {
            if entry.name == target_name {
                if clause_idx == target_clause {
                    let body = (*entry.body).clone();
                    let mut source_map = (*entry.source_map).clone();
                    // Populate file_id so BodySourceMap can produce
                    // InFile<Span> via expr_syntax_in_file / pat_syntax_in_file.
                    if source_map.file_id.is_none() {
                        source_map.file_id = Some(file_id);
                    }
                    return ArcBodyWithSourceMap(Arc::new((body, source_map)));
                }
                clause_idx += 1;
            }
        }
    }

    // Fallback: empty body (should not happen for valid CallableIds)
    let mut fallback_map = BodySourceMap::default();
    fallback_map.file_id = Some(file_id);
    ArcBodyWithSourceMap(Arc::new((Body::empty(), fallback_map)))
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
        assert!(bodies.0.len() > 0, "should have at least one body");
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
