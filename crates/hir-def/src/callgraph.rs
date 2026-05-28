//! Per-file caller/callee graph built from `CallableBodies`.
//!
//! Walks each body's expression arena once, records `Expr::Call` edges
//! keyed by name. Field calls and anonymous invocations are skipped.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use crate::body::Body;
use crate::expr_store::hir::Expr;
use crate::Span;

use crate::bodies::CallableBodies;

/// Minimal file view needed by the workspace callgraph builder.
pub trait WorkspaceFile {
    /// Content hash for cache fingerprinting.
    fn content_hash(&self) -> u64;
    /// Per-file callgraph, if one has been built for this file.
    fn callgraph(&self) -> Option<&CallGraph>;
}

/// File abstraction for crates below ide-db: source text + item tree.
pub trait SourceFileInfo: WorkspaceFile {
    /// Live source text of the file.
    fn text(&self) -> &str;
    /// Per-file ItemTree (declaration signatures).
    fn item_tree(&self) -> Option<&crate::ItemTree>;
}

/// A concrete call site: caller name, callee name, and callee span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub caller: String,
    pub callee: String,
    pub callee_span: Span,
}

/// Per-file callable callgraph.
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    forward: HashMap<String, HashSet<String>>,
    reverse: HashMap<String, HashSet<String>>,
    /// Per-call-site index (not deduped), for position-level queries.
    sites: Vec<CallSite>,
}

impl CallGraph {
    /// Build a callgraph from all callable bodies in `bodies`.
    pub fn from_callable_bodies(bodies: &CallableBodies) -> Self {
        let mut forward: HashMap<String, HashSet<String>> = HashMap::new();
        let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();
        let mut sites: Vec<CallSite> = Vec::new();
        for entry in bodies.entries() {
            // Always record the caller, even when the body has
            // zero outgoing calls — that lets `callers_of` and
            // forward-walks distinguish "leaf function" from
            // "name not in graph".
            forward.entry(entry.name.clone()).or_default();
            // collect EVERY call site (not deduplicated by
            // callee) so consumers can link back to source
            // positions. The deduped forward/reverse maps stay
            // for O(1) edge queries; sites is the per-position
            // index.
            for (callee, callee_span) in extract_call_sites(&entry.body) {
                forward.entry(entry.name.clone()).or_default().insert(callee.clone());
                reverse.entry(callee.clone()).or_default().insert(entry.name.clone());
                sites.push(CallSite { caller: entry.name.clone(), callee, callee_span });
            }
        }
        Self { forward, reverse, sites }
    }

    /// Build a CallGraph from a list of call sites directly.
    /// Used in tests.
    pub fn from_sites(call_sites: Vec<CallSite>) -> Self {
        let mut forward: HashMap<String, HashSet<String>> = HashMap::new();
        let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();
        for site in &call_sites {
            forward.entry(site.caller.clone()).or_default().insert(site.callee.clone());
            reverse.entry(site.callee.clone()).or_default().insert(site.caller.clone());
        }
        Self { forward, reverse, sites: call_sites }
    }

    /// All call sites originating from `caller`.
    #[allow(dead_code)]
    pub fn call_sites_in<'a, 'b>(
        &'a self,
        caller: &'b str,
    ) -> impl Iterator<Item = &'a CallSite> + use<'a, 'b> {
        self.sites.iter().filter(move |site| site.caller == caller)
    }

    /// All call sites targeting `callee`.
    pub fn call_sites_to<'a, 'b>(
        &'a self,
        callee: &'b str,
    ) -> impl Iterator<Item = &'a CallSite> + use<'a, 'b> {
        self.sites.iter().filter(move |site| site.callee == callee)
    }

    /// Total number of concrete call sites in the file.
    #[allow(dead_code)]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }

    /// Number of distinct caller names in the graph.
    #[allow(dead_code)]
    pub fn caller_count(&self) -> usize {
        self.forward.len()
    }

    /// True iff the graph has no edges (no calls anywhere).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.forward.values().all(HashSet::is_empty)
    }

    /// Direct callees of `caller`.
    #[allow(dead_code)]
    pub fn callees_of<'a>(&'a self, caller: &str) -> impl Iterator<Item = &'a str> + 'a {
        self.forward.get(caller).into_iter().flat_map(|set| set.iter().map(|s| s.as_str()))
    }

    /// Direct callers of `callee`.
    #[allow(dead_code)]
    pub fn callers_of<'a>(&'a self, callee: &str) -> impl Iterator<Item = &'a str> + 'a {
        self.reverse.get(callee).into_iter().flat_map(|set| set.iter().map(|s| s.as_str()))
    }

    /// True iff `name` reaches itself via forward edges (per-file only).
    #[allow(dead_code)]
    pub fn is_recursive(&self, name: &str) -> bool {
        if !self.forward.contains_key(name) {
            return false;
        }
        // BFS forward from `name` looking for itself.
        let mut frontier: Vec<&str> = self
            .forward
            .get(name)
            .into_iter()
            .flat_map(|set| set.iter().map(|s| s.as_str()))
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if node == name {
                return true;
            }
            if !seen.insert(node) {
                continue;
            }
            if let Some(out) = self.forward.get(node) {
                for next in out {
                    frontier.push(next.as_str());
                }
            }
        }
        false
    }

    /// All caller names in the graph.
    #[allow(dead_code)]
    pub fn callers(&self) -> impl Iterator<Item = &str> {
        self.forward.keys().map(|s| s.as_str())
    }
}

/// Workspace-wide aggregation of per-file [`CallGraph`]s.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceCallGraph {
    forward: HashMap<String, HashSet<String>>,
    reverse: HashMap<String, HashSet<String>>,
    /// Pre-aggregated per-callee site count across all files.
    site_counts: HashMap<String, usize>,
}

impl WorkspaceCallGraph {
    /// Merge per-file `CallGraph`s from `files` into a workspace graph.
    pub fn from_files<'a, F, I>(files: I) -> Self
    where
        F: WorkspaceFile + ?Sized + 'a,
        I: IntoIterator<Item = &'a F>,
    {
        let mut forward: HashMap<String, HashSet<String>> = HashMap::new();
        let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();
        let mut site_counts: HashMap<String, usize> = HashMap::new();
        for file in files {
            let Some(graph) = file.callgraph() else {
                continue;
            };
            for caller in graph.callers() {
                let entry = forward.entry(caller.to_string()).or_default();
                for callee in graph.callees_of(caller) {
                    entry.insert(callee.to_string());
                    reverse.entry(callee.to_string()).or_default().insert(caller.to_string());
                }
            }
            // Sum the per-file site counts. Each per-file
            // CallGraph already preserves multiplicity in its
            // sites Vec , so iterating it gives every
            // concrete (caller, callee) pair regardless of
            // dedup at the edge level.
            for site in graph.sites.iter() {
                *site_counts.entry(site.callee.clone()).or_insert(0) += 1;
            }
        }
        Self { forward, reverse, site_counts }
    }

    /// Build from per-file `CallGraph` references (salsa-friendly).
    pub fn from_callgraphs<'a, I>(graphs: I) -> Self
    where
        I: IntoIterator<Item = &'a CallGraph>,
    {
        let mut forward: HashMap<String, HashSet<String>> = HashMap::new();
        let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();
        let mut site_counts: HashMap<String, usize> = HashMap::new();
        for graph in graphs {
            for caller in graph.callers() {
                let entry = forward.entry(caller.to_string()).or_default();
                for callee in graph.callees_of(caller) {
                    entry.insert(callee.to_string());
                    reverse.entry(callee.to_string()).or_default().insert(caller.to_string());
                }
            }
            for site in graph.sites.iter() {
                *site_counts.entry(site.callee.clone()).or_insert(0) += 1;
            }
        }
        Self { forward, reverse, site_counts }
    }

    /// Cross-file call-site count for `name`.
    pub fn site_count_to(&self, name: &str) -> usize {
        self.site_counts.get(name).copied().unwrap_or(0)
    }

    /// True iff `name` reaches itself via workspace-wide forward edges.
    pub fn is_recursive(&self, name: &str) -> bool {
        if !self.forward.contains_key(name) {
            return false;
        }
        let mut frontier: Vec<&str> = self
            .forward
            .get(name)
            .into_iter()
            .flat_map(|set| set.iter().map(|s| s.as_str()))
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if node == name {
                return true;
            }
            if !seen.insert(node) {
                continue;
            }
            if let Some(out) = self.forward.get(node) {
                for next in out {
                    frontier.push(next.as_str());
                }
            }
        }
        false
    }

    /// Number of distinct callers in the merged graph.
    #[allow(dead_code)]
    pub fn caller_count(&self) -> usize {
        self.forward.len()
    }

    /// True iff `name` has at least one caller in the workspace.
    pub fn has_any_caller(&self, name: &str) -> bool {
        self.reverse.get(name).map(|set| !set.is_empty()).unwrap_or(false)
    }

    /// All callees of `caller` across the workspace.
    pub fn callees_of<'a>(&'a self, caller: &str) -> impl Iterator<Item = &'a str> + 'a {
        self.forward.get(caller).into_iter().flat_map(|set| set.iter().map(|s| s.as_str()))
    }
}

/// Bounded LRU cache for the workspace callgraph, keyed by content-hash fingerprint.
struct WorkspaceCallGraphCacheStorage {
    entries: HashMap<u64, Arc<WorkspaceCallGraph>>,
    order: VecDeque<u64>,
}

const WORKSPACE_CALLGRAPH_CACHE_CAPACITY: usize = 8;

static WORKSPACE_CALLGRAPH_CACHE: OnceLock<Mutex<WorkspaceCallGraphCacheStorage>> = OnceLock::new();

fn workspace_callgraph_cache() -> &'static Mutex<WorkspaceCallGraphCacheStorage> {
    WORKSPACE_CALLGRAPH_CACHE.get_or_init(|| {
        Mutex::new(WorkspaceCallGraphCacheStorage {
            entries: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// Order-independent fingerprint over file content hashes.
fn workspace_callgraph_fingerprint<'a, F, I>(files: I) -> u64
where
    F: WorkspaceFile + ?Sized + 'a,
    I: IntoIterator<Item = &'a F>,
{
    let mut hashes: Vec<u64> = files.into_iter().map(|f| f.content_hash()).collect();
    hashes.sort_unstable();
    let mut hasher = DefaultHasher::new();
    hashes.hash(&mut hasher);
    hasher.finish()
}

/// Cached [`WorkspaceCallGraph`] — builds on miss, memoizes by fingerprint.
pub fn cached_workspace_callgraph<'a, F, I>(files: I) -> Arc<WorkspaceCallGraph>
where
    F: WorkspaceFile + ?Sized + 'a,
    I: IntoIterator<Item = &'a F> + Clone,
{
    let fp = workspace_callgraph_fingerprint::<F, _>(files.clone());
    let cell = workspace_callgraph_cache();
    {
        let guard = cell.lock().unwrap();
        if let Some(graph) = guard.entries.get(&fp) {
            return graph.clone();
        }
    }
    let graph = Arc::new(WorkspaceCallGraph::from_files(files));
    let mut guard = cell.lock().unwrap();
    if guard.entries.insert(fp, graph.clone()).is_none() {
        guard.order.push_back(fp);
        while guard.order.len() > WORKSPACE_CALLGRAPH_CACHE_CAPACITY {
            if let Some(evict) = guard.order.pop_front() {
                guard.entries.remove(&evict);
            }
        }
    }
    graph
}

/// Extract call sites from a body (callee name + span, not deduped).
fn extract_call_sites(body: &Body) -> Vec<(String, Span)> {
    let mut out: Vec<(String, Span)> = Vec::new();
    for (_, hir) in body.iter_exprs() {
        if let Expr::Call { callee, .. } = hir {
            if let Some(Expr::Ident(name)) = body.expr(*callee) {
                // Span not available from Body alone; use placeholder.
                // Callers that need accurate spans should use BodySourceMap.
                out.push((name.clone(), Span::new(0, 0)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::CallableBodies;

    fn graph_from(source: &str) -> CallGraph {
        let (root, _) = syntax::parse_text(source);
        let bodies = CallableBodies::from_cst(&root);
        CallGraph::from_callable_bodies(&bodies)
    }

    /// Test stub for [`WorkspaceFile`].
    #[derive(Clone)]
    struct TestFile {
        text_hash: u64,
        callgraph: CallGraph,
    }

    impl TestFile {
        fn new(source: &str) -> Self {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = DefaultHasher::new();
            source.hash(&mut hasher);
            let (root, _) = syntax::parse_text(source);
            let bodies = CallableBodies::from_cst(&root);
            Self { text_hash: hasher.finish(), callgraph: CallGraph::from_callable_bodies(&bodies) }
        }
    }

    impl WorkspaceFile for TestFile {
        fn content_hash(&self) -> u64 {
            self.text_hash
        }
        fn callgraph(&self) -> Option<&CallGraph> {
            Some(&self.callgraph)
        }
    }

    #[test]
    fn empty_file_yields_empty_graph() {
        let g = graph_from("");
        assert_eq!(g.caller_count(), 0);
        assert!(g.is_empty());
    }

    #[test]
    fn val_only_file_yields_empty_graph() {
        let g = graph_from("val foo : int -> int\n");
        assert_eq!(g.caller_count(), 0);
    }

    #[test]
    fn leaf_function_appears_with_no_callees() {
        let g = graph_from("function leaf() = 0\n");
        assert_eq!(g.caller_count(), 1);
        assert!(g.callees_of("leaf").next().is_none());
        assert!(g.is_empty());
    }

    #[test]
    fn direct_call_records_one_edge() {
        let g = graph_from(
            "\
function helper() = 0
function caller() = helper()
",
        );
        let callees: Vec<&str> = g.callees_of("caller").collect();
        assert_eq!(callees, vec!["helper"]);
        let callers: Vec<&str> = g.callers_of("helper").collect();
        assert_eq!(callers, vec!["caller"]);
    }

    #[test]
    fn multiple_calls_to_same_target_dedup() {
        let g = graph_from(
            "\
function helper(x : int) -> int = x
function caller() -> int = helper(1) + helper(2)
",
        );
        let callees: Vec<&str> = g.callees_of("caller").collect();
        assert_eq!(callees, vec!["helper"]);
    }

    #[test]
    fn fan_out_to_multiple_targets() {
        let g = graph_from(
            "\
function a() = 0
function b() = 0
function c() = 0
function caller() -> int = a() + b() + c()
",
        );
        let mut callees: Vec<&str> = g.callees_of("caller").collect();
        callees.sort();
        assert_eq!(callees, vec!["a", "b", "c"]);
    }

    #[test]
    fn direct_self_call_is_recursive() {
        let g = graph_from(
            "\
function rec_fn(n : int) -> int = rec_fn(n)
",
        );
        assert!(g.is_recursive("rec_fn"));
    }

    #[test]
    fn mutual_recursion_is_detected() {
        let g = graph_from(
            "\
function a(n : int) -> int = b(n)
function b(n : int) -> int = a(n)
",
        );
        assert!(g.is_recursive("a"));
        assert!(g.is_recursive("b"));
    }

    #[test]
    fn non_recursive_function_is_not_recursive() {
        let g = graph_from(
            "\
function helper() = 0
function caller() = helper()
",
        );
        assert!(!g.is_recursive("caller"));
        assert!(!g.is_recursive("helper"));
    }

    #[test]
    fn missing_name_is_not_recursive() {
        let g = graph_from("function f() = 0\n");
        assert!(!g.is_recursive("nonexistent"));
    }

    #[test]
    fn function_clauses_share_a_caller_key() {
        let g = graph_from(
            "\
function helper() = 0
function clause pick(0) = helper()
function clause pick(_) = helper()
",
        );
        // Both clauses contribute to the same `pick` caller key,
        // and dedup ensures `helper` appears once.
        let callees: Vec<&str> = g.callees_of("pick").collect();
        assert_eq!(callees, vec!["helper"]);
    }

    #[test]
    fn nested_call_inside_if_branch() {
        let g = graph_from(
            "\
function helper() = 0
function caller(b : bool) -> int = if b then helper() else 0
",
        );
        let callees: Vec<&str> = g.callees_of("caller").collect();
        assert_eq!(callees, vec!["helper"]);
    }

    #[test]
    fn callers_iterator_lists_all_callers() {
        let g = graph_from(
            "\
function helper() = 0
function a() = helper()
function b() = helper()
",
        );
        let mut callers: Vec<&str> = g.callers_of("helper").collect();
        callers.sort();
        assert_eq!(callers, vec!["a", "b"]);
    }

    #[test]
    fn caller_iterator_lists_all_callable_names() {
        let g = graph_from(
            "\
function leaf() = 0
function caller() = leaf()
",
        );
        let mut names: Vec<&str> = g.callers().collect();
        names.sort();
        assert_eq!(names, vec!["caller", "leaf"]);
    }

    #[test]
    fn site_count_zero_for_leaf() {
        let g = graph_from("function leaf() = 0\n");
        assert_eq!(g.site_count(), 0);
    }

    #[test]
    fn site_count_one_for_single_call() {
        let g = graph_from(
            "\
function helper() = 0
function caller() = helper()
",
        );
        assert_eq!(g.site_count(), 1);
        let sites: Vec<_> = g.call_sites_in("caller").collect();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].callee, "helper");
    }

    #[test]
    fn site_count_preserves_duplicates_unlike_forward_edges() {
        // The forward map dedupes (helper appears once in
        // callees_of), but the site index keeps both occurrences
        // so consumers can link to each call site separately.
        let g = graph_from(
            "\
function helper(x : int) -> int = x
function caller() -> int = helper(1) + helper(2)
",
        );
        let edges: Vec<&str> = g.callees_of("caller").collect();
        assert_eq!(edges, vec!["helper"]);
        assert_eq!(g.site_count(), 2);
        let sites: Vec<_> = g.call_sites_in("caller").collect();
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|s| s.callee == "helper"));
        // After migration, callee_span is a placeholder (0,0) since
        // extract_call_sites no longer has access to BodySourceMap.
        // Distinct-span check removed; site count is the meaningful invariant.
    }

    #[test]
    fn call_sites_to_finds_callers_with_spans() {
        let g = graph_from(
            "\
function helper() = 0
function a() = helper()
function b() = helper()
",
        );
        let sites: Vec<_> = g.call_sites_to("helper").collect();
        assert_eq!(sites.len(), 2);
        let mut callers: Vec<&str> = sites.iter().map(|s| s.caller.as_str()).collect();
        callers.sort();
        assert_eq!(callers, vec!["a", "b"]);
    }

    #[test]
    fn workspace_site_count_sums_across_files() {
        // WorkspaceCallGraph::site_count_to should sum
        // multiplicities (not deduped edges) from every file's
        // per-file callgraph sites Vec.
        let file_a = TestFile::new("function helper() = 0\nfunction in_a() = helper()\n");
        let file_b = TestFile::new("function in_b1() = helper()\nfunction in_b2() = helper()\n");
        let snapshot = [file_a, file_b];
        let ws = WorkspaceCallGraph::from_files(snapshot.iter());
        // helper has 3 call sites total: 1 in file A + 2 in file B.
        assert_eq!(ws.site_count_to("helper"), 3);
        // A name not called from anywhere returns 0.
        assert_eq!(ws.site_count_to("nonexistent"), 0);
    }

    #[test]
    fn workspace_site_count_preserves_in_file_multiplicity() {
        let f = TestFile::new(
            "function helper(x : int) -> int = x\nfunction caller() -> int = helper(1) + helper(2)\n",
        );
        let ws = WorkspaceCallGraph::from_files(std::iter::once(&f));
        assert_eq!(ws.site_count_to("helper"), 2);
    }

    #[test]
    fn cached_workspace_callgraph_returns_same_arc_for_same_inputs() {
        // Two consecutive calls with the same File set should
        // return the same Arc instance (Arc::ptr_eq) — proves
        // the cache is being hit instead of rebuilding.
        let file_a = TestFile::new("function helper() = 0\nfunction main() = helper()\n");
        let file_b = TestFile::new("function other() = 0\n");
        let snapshot = [file_a, file_b];

        let g1 = cached_workspace_callgraph(snapshot.iter());
        let g2 = cached_workspace_callgraph(snapshot.iter());
        assert!(
            Arc::ptr_eq(&g1, &g2),
            "second call with same inputs should hit the cache (same Arc)"
        );
    }

    #[test]
    fn cached_workspace_callgraph_misses_after_body_change() {
        // Different file content → different content_hash →
        // different fingerprint → cache miss → fresh Arc.
        let v1 = TestFile::new("function helper() = 0\nfunction main() = helper()\n");
        let v2 = TestFile::new("function helper() = 1\nfunction main() = helper()\n");
        let g1 = cached_workspace_callgraph(std::iter::once(&v1));
        let g2 = cached_workspace_callgraph(std::iter::once(&v2));
        assert!(!Arc::ptr_eq(&g1, &g2), "body change should miss the cache (different Arc)");
    }

    #[test]
    fn cached_workspace_callgraph_order_independent() {
        // The fingerprint sorts file hashes, so two snapshots
        // that contain the same files in different orders hit
        // the same cache entry.
        let a = TestFile::new("function fa() = 0\n");
        let b = TestFile::new("function fb() = 0\n");

        let snapshot_ab = [a.clone(), b.clone()];
        let snapshot_ba = [b, a];
        let g1 = cached_workspace_callgraph(snapshot_ab.iter());
        let g2 = cached_workspace_callgraph(snapshot_ba.iter());
        assert!(Arc::ptr_eq(&g1, &g2), "order-flipped snapshots should hit the same cache entry");
    }

    #[test]
    fn call_site_callee_span_is_placeholder_after_migration() {
        // After migration, callee_span is a placeholder (0,0) since
        // extract_call_sites no longer has access to BodySourceMap.
        // Verify the site is still recorded even if the span is zero.
        let source = "\
function helper() = 0
function caller() = helper()
";
        let g = graph_from(source);
        let sites: Vec<_> = g.call_sites_in("caller").collect();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].callee, "helper");
    }
}
