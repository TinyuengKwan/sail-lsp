//! Include graph visualization.
//!
//! Sail-specific: renders the $include dependency graph as
//! readable Markdown text. Shows which files include which others,
//! with transitive closure information.

use base_db::FileId;
use hir_def::include_graph::IncludeGraph;
use std::collections::HashMap;

/// Render the include graph as readable text.
///
/// `file_name`: callback to get display name for each FileId.
pub fn render_include_graph(graph: &IncludeGraph, file_name: &dyn Fn(FileId) -> String) -> String {
    let all_files = graph.all_files();
    if all_files.is_empty() {
        return "(no include relationships)".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!("## Include Graph ({} files)\n", all_files.len()));

    // Collect edges and group by source
    let mut edges: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for &fid in &all_files {
        let includes = graph.includes_of(fid);
        if !includes.is_empty() {
            edges.insert(fid, includes.to_vec());
        }
    }

    // Render as tree
    for (source, targets) in &edges {
        let source_name = file_name(*source);
        for target in targets {
            let target_name = file_name(*target);
            lines.push(format!("- **{}** → {}", source_name, target_name));
        }
    }

    // Show topological order if available
    if let Some(order) = graph.topological_order() {
        lines.push(String::new());
        lines.push("### Processing Order".to_string());
        for (idx, fid) in order.iter().enumerate() {
            lines.push(format!("{}. {}", idx + 1, file_name(*fid)));
        }
    }

    // Show files with no includes (leaf files)
    let roots: Vec<FileId> = all_files
        .iter()
        .filter(|f| graph.includes_of(**f).is_empty() && !graph.included_by(**f).is_empty())
        .copied()
        .collect();
    if !roots.is_empty() {
        lines.push(String::new());
        lines.push("### Leaf Files (included but don't include others)".to_string());
        for fid in roots {
            lines.push(format!("- {}", file_name(fid)));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_simple_graph() {
        let mut graph = IncludeGraph::new();
        graph.add_edge(FileId::from_raw(0), FileId::from_raw(1));
        graph.add_edge(FileId::from_raw(0), FileId::from_raw(2));

        let output = render_include_graph(&graph, &|fid| format!("file_{}.sail", fid.index()));
        assert!(output.contains("file_0.sail"), "should show source");
        assert!(output.contains("file_1.sail"), "should show target");
        assert!(output.contains("Include Graph"));
    }

    #[test]
    fn render_empty_graph() {
        let graph = IncludeGraph::new();
        let output = render_include_graph(&graph, &|_| "x".to_string());
        assert!(output.contains("no include"));
    }
}
