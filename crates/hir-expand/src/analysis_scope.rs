//! Include-aware analysis scope.
//!
//! Analogous to a crate graph: defines which files can see
//! which other files' symbols. For Sail, this is determined by
//! `$include` directives — a file sees its own definitions plus
//! the transitive closure of all `$include`d files.

use std::collections::HashSet;

use base_db::FileId;

use crate::include_graph::IncludeGraph;

/// Analysis scope for a single file: the file itself plus all
/// transitively `$include`d files.
///
/// Analogous to a per-crate namespace — files within the scope
/// share a namespace, files outside are invisible.
#[derive(Debug, Clone)]
pub struct AnalysisScope {
    /// The root file being analyzed.
    pub root: FileId,
    /// All files transitively included by the root.
    pub included: HashSet<FileId>,
}

impl AnalysisScope {
    /// Build scope from an include graph for a given root file.
    pub fn from_include_graph(graph: &IncludeGraph, root: FileId) -> Self {
        Self { root, included: graph.transitive_includes(root) }
    }

    /// All files in scope (root + included).
    pub fn all_files(&self) -> impl Iterator<Item = FileId> + '_ {
        std::iter::once(self.root).chain(self.included.iter().copied())
    }

    /// Check if a file is within this analysis scope.
    pub fn contains(&self, file: FileId) -> bool {
        file == self.root || self.included.contains(&file)
    }

    /// Number of files in scope.
    pub fn len(&self) -> usize {
        1 + self.included.len()
    }

    /// Always false — an analysis scope always contains at least the root file.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Scope with no includes (just the root file).
    pub fn single_file(root: FileId) -> Self {
        Self { root, included: HashSet::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::include_graph::IncludeGraph;

    #[test]
    fn scope_from_empty_graph() {
        let graph = IncludeGraph::new();
        let scope = AnalysisScope::from_include_graph(&graph, FileId::from_raw(0));
        assert_eq!(scope.len(), 1);
        assert!(scope.contains(FileId::from_raw(0)));
        assert!(!scope.contains(FileId::from_raw(1)));
    }

    #[test]
    fn scope_transitive() {
        let mut graph = IncludeGraph::new();
        graph.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        graph.add_edge(FileId::from_raw(1), FileId::from_raw(2));
        let scope = AnalysisScope::from_include_graph(&graph, FileId::from_raw(0));
        assert_eq!(scope.len(), 3);
        assert!(scope.contains(FileId::from_raw(0)));
        assert!(scope.contains(FileId::from_raw(1)));
        assert!(scope.contains(FileId::from_raw(2)));
    }

    #[test]
    fn scope_excludes_non_included() {
        let mut graph = IncludeGraph::new();
        graph.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        graph.add_edge(FileId::from_raw(2), FileId::from_raw(3)); // separate chain
        let scope = AnalysisScope::from_include_graph(&graph, FileId::from_raw(0));
        assert!(scope.contains(FileId::from_raw(1)));
        assert!(!scope.contains(FileId::from_raw(2)));
        assert!(!scope.contains(FileId::from_raw(3)));
    }
}
