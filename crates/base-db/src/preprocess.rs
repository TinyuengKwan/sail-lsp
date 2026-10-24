//! Build configuration and per-file conditional-compilation projections.
use crate::{module_graph::ModuleGraph, FileId, FileText};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceProject {
    pub roots: Vec<FileId>,
    /// Explicit owners in root order; empty for legacy callers with unique roots.
    pub root_modules: Vec<Option<usize>>,
    pub paths: Vec<(PathBuf, FileId)>,
    pub library: Option<PathBuf>,
    pub include_paths: Vec<PathBuf>,
    pub target: Option<String>,
    pub modules: ModuleGraph,
}

#[salsa::input(singleton)]
pub struct WorkspaceSourceProject {
    #[returns(ref)]
    pub project: SourceProject,
    #[returns(ref)]
    pub files: BTreeMap<FileId, FileText>,
}

/// Stable include occurrence: root/module plus include ordinals along the path.
/// Function edits and byte-offset changes do not change this identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceKey {
    pub root: FileId,
    pub occurrence: usize,
    pub module: Option<usize>,
    pub includes: Vec<usize>,
}

#[salsa::input(singleton)]
pub struct WorkspacePreprocessing {
    #[returns(ref)]
    pub regions: BTreeMap<FileId, Vec<bool>>,
    #[returns(ref)]
    pub sources: BTreeMap<FileId, FileText>,
    #[returns(ref)]
    pub instances: BTreeMap<FileId, Vec<FileId>>,
    /// Active source regions in compiler expansion order. Includes split a
    /// parent file into several entries; a file-level postorder is insufficient.
    /// Region ordinals, unlike byte offsets, are stable under body edits.
    #[returns(ref)]
    pub order: Vec<(FileId, usize)>,
}

/// `None` is a legacy analysis without an explicit project model. Once a
/// project model exists, only files reached by its expansion are members;
/// loading a file into the VFS does not make it part of that project.
pub fn project_membership(db: &dyn salsa::Database, file: FileId) -> Option<bool> {
    WorkspaceSourceProject::try_get(db)?;
    let source = physical_file(db, file);
    Some(
        WorkspacePreprocessing::try_get(db)
            .is_some_and(|input| is_project_member(db, input, source)),
    )
}

#[salsa::tracked]
fn is_project_member(
    db: &dyn salsa::Database,
    input: WorkspacePreprocessing,
    source: FileId,
) -> bool {
    input.instances(db).contains_key(&source)
}

pub fn file_regions(db: &dyn salsa::Database, file: FileId) -> Option<&Vec<bool>> {
    if project_membership(db, file) == Some(false) {
        return None;
    }
    let input = WorkspacePreprocessing::try_get(db)?;
    projected_regions(db, input, file).as_ref()
}

#[salsa::tracked(returns(ref))]
fn projected_regions(
    db: &dyn salsa::Database,
    input: WorkspacePreprocessing,
    file: FileId,
) -> Option<Vec<bool>> {
    input.regions(db).get(&file).cloned()
}

/// Source identity for locations and shared raw parsing.
pub fn physical_file(db: &dyn salsa::Database, id: FileId) -> FileId {
    source_text(db, id).map_or(id, |source| source.file_id(db))
}

pub fn source_text(db: &dyn salsa::Database, id: FileId) -> Option<FileText> {
    if !id.is_instance() {
        return None;
    }
    let input = WorkspacePreprocessing::try_get(db)?;
    projected_source(db, input, id)
}

#[salsa::tracked]
fn projected_source(
    db: &dyn salsa::Database,
    input: WorkspacePreprocessing,
    id: FileId,
) -> Option<FileText> {
    input.sources(db).get(&id).copied()
}

pub fn file_instances(db: &dyn salsa::Database, id: FileId) -> Vec<FileId> {
    if id.is_instance() {
        return vec![id];
    }
    WorkspacePreprocessing::try_get(db)
        .and_then(|input| input.instances(db).get(&id).cloned())
        .unwrap_or_else(|| vec![id])
}

pub fn set_preprocessing(
    db: &mut dyn salsa::Database,
    regions: BTreeMap<FileId, Vec<bool>>,
    sources: BTreeMap<FileId, FileText>,
    instances: BTreeMap<FileId, Vec<FileId>>,
    order: Vec<(FileId, usize)>,
) {
    use salsa::Setter;
    match WorkspacePreprocessing::try_get(db) {
        Some(input) => {
            if input.regions(db) != &regions {
                input.set_regions(db).to(regions);
            }
            if input.sources(db) != &sources {
                input.set_sources(db).to(sources);
            }
            if input.instances(db) != &instances {
                input.set_instances(db).to(instances);
            }
            if input.order(db) != &order {
                input.set_order(db).to(order);
            }
        }
        None => {
            WorkspacePreprocessing::new(db, regions, sources, instances, order);
        }
    }
}
