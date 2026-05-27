//! Expand $include — preview included content.
//!
//! of a preprocessor directive. For Sail, this means showing what
//! files are transitively included and their contents.
//!
//! Used as a custom LSP command `sail-lsp/expandInclude`.

use base_db::FileId;
use hir_def::include_graph::IncludeGraph;

/// Result of expanding includes for a file.
#[derive(Debug, Clone)]
pub struct ExpandIncludeResult {
    /// The root file's name/path.
    pub root: String,
    /// Combined expansion text showing all included content.
    pub expansion: String,
}

/// Expand all $include directives for a file, showing the transitive
/// closure of included file contents.
///
/// `file_text`: callback to get text for a FileId.
/// `file_name`: callback to get display name for a FileId.
pub fn expand_includes(
    graph: &IncludeGraph,
    root: FileId,
    file_text: &dyn Fn(FileId) -> Option<String>,
    file_name: &dyn Fn(FileId) -> String,
) -> ExpandIncludeResult {
    let root_name = file_name(root);
    let mut expansion = String::new();

    // Show root file content
    if let Some(text) = file_text(root) {
        expansion.push_str(&format!("// === {} ===\n", root_name));
        expansion.push_str(&text);
        expansion.push('\n');
    }

    // Show transitively included files
    let included = graph.transitive_includes(root);
    if !included.is_empty() {
        expansion.push_str(&format!("\n// --- {} file(s) included ---\n\n", included.len()));

        // Use topological order if available
        let ordered: Vec<FileId> = graph
            .topological_order()
            .map(|order| order.into_iter().filter(|f| included.contains(f)).collect())
            .unwrap_or_else(|| included.into_iter().collect());

        for fid in ordered {
            let name = file_name(fid);
            if let Some(text) = file_text(fid) {
                expansion.push_str(&format!("// === $include {} ===\n", name));
                expansion.push_str(&text);
                expansion.push_str("\n\n");
            }
        }
    }

    ExpandIncludeResult { root: root_name, expansion }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn expand_single_include() {
        let mut graph = IncludeGraph::new();
        graph.add_edge(FileId::from_raw(0), FileId::from_raw(1));

        let texts: HashMap<FileId, String> = [
            (
                FileId::from_raw(0),
                "// main\n$include \"lib.sail\"\nfunction main() = 1\n".to_string(),
            ),
            (FileId::from_raw(1), "val helper : int -> int\n".to_string()),
        ]
        .into();

        let names: HashMap<FileId, String> = [
            (FileId::from_raw(0), "main.sail".to_string()),
            (FileId::from_raw(1), "lib.sail".to_string()),
        ]
        .into();

        let result =
            expand_includes(&graph, FileId::from_raw(0), &|fid| texts.get(&fid).cloned(), &|fid| {
                names.get(&fid).cloned().unwrap_or_else(|| format!("{:?}", fid))
            });

        assert!(result.expansion.contains("main.sail"));
        assert!(result.expansion.contains("lib.sail"));
        assert!(result.expansion.contains("val helper"));
    }

    #[test]
    fn expand_no_includes() {
        let graph = IncludeGraph::new();

        let result = expand_includes(
            &graph,
            FileId::from_raw(0),
            &|_| Some("val x : int\n".to_string()),
            &|_| "file.sail".to_string(),
        );

        assert!(result.expansion.contains("val x : int"));
        assert!(!result.expansion.contains("included"));
    }
}
