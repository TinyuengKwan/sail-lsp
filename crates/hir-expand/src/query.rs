//! Include-related salsa tracked queries.
//!
//! Extracted from `hir-def/src/def_query.rs` — these belong in hir-expand
//! because they depend only on `base_db::FileText` and `salsa`, and they
//! drive the include graph which is an expand-layer concern.

use std::sync::Arc;

use base_db::FileText;

use crate::include_graph::IncludeGraph;

/// An include path with its resolution type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IncludePath {
    /// `$include "relative/path.sail"` — resolved relative to current file.
    Relative(String),
    /// `$include <lib/path.sail>` — resolved in $SAIL_DIR/lib/.
    Library(String),
}

#[salsa::tracked(returns(ref))]
pub fn include_paths(db: &dyn salsa::Database, input: FileText) -> Vec<String> {
    typed_include_paths(db, input)
        .iter()
        .map(|p| match p {
            IncludePath::Relative(s) | IncludePath::Library(s) => s.clone(),
        })
        .collect()
}

/// Typed version of include_paths that preserves the Relative/Library
/// distinction for proper path resolution.
#[salsa::tracked(returns(ref))]
pub fn typed_include_paths(db: &dyn salsa::Database, input: FileText) -> Vec<IncludePath> {
    let text = input.text(db);
    let mut includes = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("$include") {
            let rest = rest.trim();
            if let Some(path) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                includes.push(IncludePath::Relative(path.to_string()));
            } else if let Some(path) = rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
                includes.push(IncludePath::Library(path.to_string()));
            }
        }
    }
    includes
}

/// Newtype wrapper for `Arc<IncludeGraph>` with pointer-based Eq/Hash.
#[derive(Clone, Debug)]
pub struct ArcIncludeGraph(pub Arc<IncludeGraph>);

impl PartialEq for ArcIncludeGraph {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcIncludeGraph {}
impl std::hash::Hash for ArcIncludeGraph {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

/// Salsa input: the workspace include graph, set once after workspace scan.
#[salsa::input(singleton)]
pub struct WorkspaceIncludeGraph {
    #[returns(ref)]
    pub graph: ArcIncludeGraph,
}
