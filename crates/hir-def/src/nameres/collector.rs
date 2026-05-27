//! Definition collector — builds a `DefMap` by walking an `ItemTree`
//! and resolving `$include` dependencies.
//! Sail's simplified version:
//!   - No macros to expand
//!   - No glob imports
//!   - `$include` is analogous to `use` / `extern crate`
//!   - Fixed-point loop resolves include chains
//!
//! The collector produces a `DefMap` that correctly reflects the
//! include-order-dependent shadowing semantics of Sail.

use std::sync::Arc;

use crate::include_graph::IncludeGraph;
use crate::item_tree::{ItemKind, ItemTree};
use crate::per_ns::Namespace;

use super::DefMap;

/// Walks the item tree and include graph to build a complete `DefMap`.
///
/// For Sail, this handles:
/// - Collecting all definitions from the root file's ItemTree
/// - Resolving `$include` directives to pull in definitions from
///   included files (respecting include order for shadowing)
pub(crate) struct DefCollector<'a> {
    /// The DefMap being built.  All definitions are added to the root
    /// module's scope via `def_map.root_scope_mut()`.
    ///
    /// `def_map.modules[root].scope`.
    def_map: DefMap,
    /// Include graph for resolving cross-file includes.
    #[allow(dead_code)] // used when workspace-level include resolution is wired
    include_graph: Option<&'a IncludeGraph>,
    /// Already-processed file indices (prevents cycles).
    #[allow(dead_code)] // used when workspace-level include resolution is wired
    processed_files: Vec<usize>,
    /// Include directive spans from the root file's ItemTree.
    /// Used to emit diagnostics with real source spans.
    include_spans: Vec<(String, crate::Span)>,
}

#[allow(dead_code)] // WIP: DefCollector will be used when workspace-level nameres is wired
impl<'a> DefCollector<'a> {
    /// Create a new collector.
    pub(crate) fn new() -> Self {
        let mut def_map = DefMap::default();
        def_map.modules[def_map.root].origin = Some(super::ModuleOrigin::File);
        Self {
            def_map,
            include_graph: None,
            processed_files: Vec::new(),
            include_spans: Vec::new(),
        }
    }

    /// Set the include graph for cross-file resolution.
    #[allow(dead_code)] // used when workspace-level include resolution is wired
    pub(crate) fn with_include_graph(mut self, graph: &'a IncludeGraph) -> Self {
        self.include_graph = Some(graph);
        self
    }

    /// Seed with a single file's ItemTree.
    pub(crate) fn seed_with_item_tree(&mut self, item_tree: &Arc<ItemTree>) {
        // Capture include spans for diagnostic reporting
        self.include_spans = item_tree.include_spans.clone();
        self.collect_from_tree(item_tree);
    }

    /// Collect definitions from included files, resolving in include order.
    #[allow(dead_code)] // used when workspace-level include resolution is wired
    pub(crate) fn collect_includes(
        &mut self,
        root_file_idx: usize,
        file_item_trees: &[(usize, &ItemTree)],
    ) {
        self.processed_files.push(root_file_idx);

        if let Some(graph) = self.include_graph {
            let file_id = base_db::FileId::from_raw(root_file_idx as u32);
            let reachable = graph.transitive_includes(file_id);

            for &included_file_id in &reachable {
                let idx = included_file_id.index() as usize;
                if self.processed_files.contains(&idx) {
                    continue;
                }
                self.processed_files.push(idx);

                if let Some((_, tree)) = file_item_trees.iter().find(|(i, _)| *i == idx) {
                    self.collect_from_tree(tree);
                } else {
                    // Emit diagnostic with real source span from ItemTree.
                    let include_span = self
                        .include_spans
                        .iter()
                        .find(|(path, _)| {
                            // Match by file index suffix in the path
                            path.contains(&format!("{}", idx))
                                || path.ends_with(&format!("file_id={}", idx))
                        })
                        .map(|(_, span)| *span)
                        .unwrap_or(parser::Span::new(0, 0));
                    self.def_map.push_diagnostic(super::DefDiagnostic::UnresolvedInclude {
                        path: format!("file_id={}", included_file_id.index()),
                        range: include_span,
                    });
                }
            }
        }
    }

    /// Add definitions from an ItemTree to the root module's scope.
    fn collect_from_tree(&mut self, item_tree: &ItemTree) {
        for (idx, &id) in item_tree.top_level_items().iter().enumerate() {
            let name = id.name(item_tree).clone();
            let kind = id.item_kind(item_tree);
            let visibility = id.visibility(item_tree);
            let ns = item_kind_to_namespace(kind);

            // Detect duplicate type definitions.
            // Scattered clauses are exempt — they accumulate into a single definition.
            let is_scattered_clause = kind == ItemKind::ScatteredClause;
            if ns == Namespace::Types && !is_scattered_clause {
                let existing = self.def_map.root_scope().get(&name);
                if let Some(first_item) = existing.types {
                    self.def_map.push_diagnostic(super::DefDiagnostic::DuplicateDefinition {
                        name: name.clone(),
                        first: super::DefId(first_item.def.as_raw()),
                        second: super::DefId(self.def_map.len() as u32),
                    });
                }
            }

            let raw_id = self.def_map.alloc_def(name.clone(), kind, visibility, idx);
            let module_def_id = crate::item_id::ModuleDefId::from_def_id(raw_id, kind);
            let scope = self.def_map.root_scope_mut();
            scope.declare(module_def_id);
            scope.push_res(name, module_def_id, ns);
        }
    }

    /// Finish collection and return the completed `DefMap`.
    pub(crate) fn finish(self) -> DefMap {
        self.def_map
    }
}

/// Map an `ItemKind` to the appropriate namespace.
fn item_kind_to_namespace(kind: ItemKind) -> Namespace {
    match kind {
        ItemKind::Struct
        | ItemKind::Union
        | ItemKind::Enum
        | ItemKind::Bitfield
        | ItemKind::Newtype
        | ItemKind::TypeAlias => Namespace::Types,
        _ => Namespace::Values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::Name;

    fn item_tree_for(source: &str) -> Arc<ItemTree> {
        let (root, _) = syntax::parse_text(source);
        Arc::new(ItemTree::build_from_cst(&root))
    }

    #[test]
    fn collector_seeds_from_item_tree() {
        let tree = item_tree_for("val foo : int\nfunction bar() = 0\n");
        let mut collector = DefCollector::new();
        collector.seed_with_item_tree(&tree);
        let def_map = collector.finish();

        assert_eq!(def_map.len(), 2);
        assert!(!def_map.lookup_name("foo").is_empty());
        assert!(!def_map.lookup_name("bar").is_empty());

        // Check namespace assignment via root_scope()
        let scope = def_map.root_scope();
        let foo_ns = scope.get(&Name::new("foo"));
        assert!(foo_ns.take_values().is_some()); // val spec → values
        let bar_ns = scope.get(&Name::new("bar"));
        assert!(bar_ns.take_values().is_some()); // function → values
    }

    #[test]
    fn collector_type_goes_to_type_namespace() {
        let tree = item_tree_for("struct Foo = { x : int }\nenum Bar = { A, B }\n");
        let mut collector = DefCollector::new();
        collector.seed_with_item_tree(&tree);
        let def_map = collector.finish();

        let scope = def_map.root_scope();
        let foo_ns = scope.get(&Name::new("Foo"));
        assert!(foo_ns.take_types().is_some());
        let bar_ns = scope.get(&Name::new("Bar"));
        assert!(bar_ns.take_types().is_some());
    }
}
