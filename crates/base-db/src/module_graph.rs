//! Semantic project membership, independent of manifest syntax and the VFS.
use crate::FileId;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleData {
    pub files: Vec<FileId>,
    /// Direct requirements, including manifest inheritance and parents.
    /// Build reachability is transitive; name visibility is not.
    pub requires: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleGraph {
    pub modules: Vec<ModuleData>,
    pub globals: Vec<FileId>,
}

#[salsa::input(singleton)]
pub struct WorkspaceModules {
    #[returns(ref)]
    pub graph: ModuleGraph,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileScope {
    pub visible: BTreeSet<FileId>,
    pub own: BTreeSet<FileId>,
}

/// A per-file projection: changing an unrelated module can backdate this
/// result without invalidating the file's name resolution or inference.
pub fn file_scope(db: &dyn salsa::Database, file: FileId) -> Option<Arc<FileScope>> {
    let input = WorkspaceModules::try_get(db)?;
    module_file_scope(db, input, file).clone()
}

#[salsa::tracked(returns(ref))]
fn module_file_scope(
    db: &dyn salsa::Database,
    input: WorkspaceModules,
    file: FileId,
) -> Option<Arc<FileScope>> {
    let graph = input.graph(db);
    if graph.modules.is_empty() {
        return None;
    }
    let mut scope = FileScope::default();
    scope.visible.extend(graph.globals.iter().copied());
    if let Some(module) = graph.modules.iter().find(|m| m.files.contains(&file)) {
        scope.own.extend(module.files.iter().copied());
        scope.visible.extend(scope.own.iter().copied());
        for &id in &module.requires {
            if let Some(required) = graph.modules.get(id) {
                scope.visible.extend(required.files.iter().copied());
            }
        }
    } else if graph.globals.contains(&file) {
        scope.own.extend(graph.globals.iter().copied());
    } else {
        // Retained inactive buffers are local, not another global module.
        scope.own.insert(file);
        scope.visible.insert(file);
    }
    Some(Arc::new(scope))
}

pub fn can_reference_file(db: &dyn salsa::Database, from: FileId, target: FileId) -> bool {
    from == target || file_scope(db, from).as_ref().is_none_or(|s| s.visible.contains(&target))
}

pub fn same_module(db: &dyn salsa::Database, from: FileId, target: FileId) -> bool {
    from == target || file_scope(db, from).as_ref().is_some_and(|s| s.own.contains(&target))
}

pub fn set_module_graph(db: &mut dyn salsa::Database, graph: ModuleGraph) {
    use salsa::Setter;
    match WorkspaceModules::try_get(db) {
        Some(input) if input.graph(db) != &graph => {
            input.set_graph(db).to(graph);
        }
        Some(_) => {}
        None => {
            WorkspaceModules::new(db, graph);
        }
    }
}
