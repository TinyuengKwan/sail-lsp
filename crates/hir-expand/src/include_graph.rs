//! Include dependency graph for `$include` directive tracking.
//!
//! Tracks which files include which other files, enabling:
//! - Reverse dependency lookup (who includes file X?)
//! - Topological ordering (process files in dependency order)
//! - Transitive include closure (all files reachable from X)

use std::collections::{HashMap, HashSet, VecDeque};

use base_db::FileId;

/// Directed graph of `$include` dependencies between files.
///
/// Edge `A → B` means file A contains `$include "B"` or `$include <B>`.
#[derive(Debug, Default, Clone)]
pub struct IncludeGraph {
    /// FileId → files it includes (in declaration order).
    includes: HashMap<FileId, Vec<FileId>>,
    /// Reverse index: FileId → files that include it.
    included_by: HashMap<FileId, Vec<FileId>>,
}

impl IncludeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `from` includes `to`.
    pub fn add_edge(&mut self, from: FileId, to: FileId) {
        self.includes.entry(from).or_default().push(to);
        self.included_by.entry(to).or_default().push(from);
    }

    /// Files directly included by `file`.
    pub fn includes_of(&self, file: FileId) -> &[FileId] {
        self.includes.get(&file).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Files that directly include `file`.
    pub fn included_by(&self, file: FileId) -> &[FileId] {
        self.included_by.get(&file).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// All files known to the graph.
    pub fn all_files(&self) -> HashSet<FileId> {
        let mut files = HashSet::new();
        for (&k, vs) in &self.includes {
            files.insert(k);
            files.extend(vs);
        }
        for (&k, vs) in &self.included_by {
            files.insert(k);
            files.extend(vs);
        }
        files
    }

    /// Topological sort of all files (includers before included files).
    /// Returns `None` if there's a cycle.
    pub fn topological_order(&self) -> Option<Vec<FileId>> {
        let all = self.all_files();
        let mut in_degree: HashMap<FileId, usize> = all.iter().map(|&f| (f, 0)).collect();

        for (_, targets) in &self.includes {
            for &t in targets {
                *in_degree.entry(t).or_default() += 1;
            }
        }

        let mut queue: VecDeque<FileId> =
            in_degree.iter().filter(|(_, &deg)| deg == 0).map(|(&f, _)| f).collect();

        let mut order = Vec::new();
        while let Some(file) = queue.pop_front() {
            order.push(file);
            for &target in self.includes_of(file) {
                if let Some(deg) = in_degree.get_mut(&target) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(target);
                    }
                }
            }
        }

        if order.len() == all.len() {
            Some(order)
        } else {
            None // cycle detected
        }
    }

    /// Find files participating in a cycle, if any.
    ///
    /// Returns `Some(cycle_files)` if there's a cycle, `None` if the
    /// graph is a DAG. The returned set contains all files that are
    /// part of at least one cycle (files with non-zero in-degree after
    /// topological sort drains all non-cyclic nodes).
    pub fn find_cycle(&self) -> Option<Vec<FileId>> {
        let all = self.all_files();
        let mut in_degree: HashMap<FileId, usize> = all.iter().map(|&f| (f, 0)).collect();

        for (_, targets) in &self.includes {
            for &t in targets {
                *in_degree.entry(t).or_default() += 1;
            }
        }

        let mut queue: VecDeque<FileId> =
            in_degree.iter().filter(|(_, &deg)| deg == 0).map(|(&f, _)| f).collect();

        let mut processed = 0;
        while let Some(file) = queue.pop_front() {
            processed += 1;
            for &target in self.includes_of(file) {
                if let Some(deg) = in_degree.get_mut(&target) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(target);
                    }
                }
            }
        }

        if processed == all.len() {
            None // No cycle
        } else {
            // Files with remaining in-degree > 0 are part of a cycle.
            let cycle_files: Vec<FileId> =
                in_degree.into_iter().filter(|(_, deg)| *deg > 0).map(|(f, _)| f).collect();
            Some(cycle_files)
        }
    }

    /// Transitive closure: all files reachable from `root` via includes.
    pub fn transitive_includes(&self, root: FileId) -> HashSet<FileId> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(root);
        while let Some(file) = queue.pop_front() {
            for &target in self.includes_of(file) {
                if visited.insert(target) {
                    queue.push_back(target);
                }
            }
        }
        visited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_graph() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(0), FileId::from_raw(2));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(3));

        assert_eq!(g.includes_of(FileId::from_raw(0)), &[FileId::from_raw(1), FileId::from_raw(2)]);
        assert_eq!(g.included_by(FileId::from_raw(1)), &[FileId::from_raw(0)]);
        assert_eq!(g.includes_of(FileId::from_raw(2)), &[]);
    }

    #[test]
    fn topological_order_linear() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(2));

        let order = g.topological_order().unwrap();
        let pos = |id: FileId| order.iter().position(|&f| f == id).unwrap();
        assert!(pos(FileId::from_raw(0)) < pos(FileId::from_raw(1)));
        assert!(pos(FileId::from_raw(1)) < pos(FileId::from_raw(2)));
    }

    #[test]
    fn topological_order_cycle() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(0)); // cycle

        assert!(g.topological_order().is_none());
    }

    #[test]
    fn find_cycle_none_for_dag() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(2));
        assert!(g.find_cycle().is_none());
    }

    #[test]
    fn find_cycle_detects_simple_cycle() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(0)); // cycle
        let cycle = g.find_cycle().unwrap();
        assert!(cycle.contains(&FileId::from_raw(0)));
        assert!(cycle.contains(&FileId::from_raw(1)));
    }

    #[test]
    fn find_cycle_detects_indirect_cycle() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(2));
        g.add_edge(FileId::from_raw(2), FileId::from_raw(0)); // cycle: 0→1→2→0
        let cycle = g.find_cycle().unwrap();
        assert_eq!(cycle.len(), 3);
        assert!(cycle.contains(&FileId::from_raw(0)));
        assert!(cycle.contains(&FileId::from_raw(1)));
        assert!(cycle.contains(&FileId::from_raw(2)));
    }

    #[test]
    fn find_cycle_excludes_non_cyclic_files() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1)); // non-cyclic
        g.add_edge(FileId::from_raw(1), FileId::from_raw(2));
        g.add_edge(FileId::from_raw(2), FileId::from_raw(1)); // cycle: 1→2→1
        let cycle = g.find_cycle().unwrap();
        // FileId::from_raw(0) is not part of the cycle
        assert!(!cycle.contains(&FileId::from_raw(0)));
        assert!(cycle.contains(&FileId::from_raw(1)));
        assert!(cycle.contains(&FileId::from_raw(2)));
    }

    #[test]
    fn transitive_includes() {
        let mut g = IncludeGraph::new();
        g.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        g.add_edge(FileId::from_raw(1), FileId::from_raw(2));
        g.add_edge(FileId::from_raw(0), FileId::from_raw(3));

        let reachable = g.transitive_includes(FileId::from_raw(0));
        assert!(reachable.contains(&FileId::from_raw(1)));
        assert!(reachable.contains(&FileId::from_raw(2)));
        assert!(reachable.contains(&FileId::from_raw(3)));
        assert!(!reachable.contains(&FileId::from_raw(0))); // root not included
    }
}
