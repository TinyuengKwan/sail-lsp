//! Stable declaration positions in the expanded program. Include instances
//! retain their identity while parent regions interleave with included files.
use base_db::{preprocess::WorkspacePreprocessing, FileText, WorkspaceFiles};
use std::collections::HashMap;

pub mod bindings;
pub mod inline;
pub mod scattered;

/// Index in the instance's semantic CST, independent of byte offsets and body
/// contents. Source locations are recovered from the current parse on demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Declaration {
    pub file: FileText,
    pub index: usize,
}

#[salsa::tracked(returns(ref))]
fn file_regions(db: &dyn salsa::Database, file: FileText) -> Vec<Vec<usize>> {
    let raw = syntax::parse_query::parse_file(db, file).syntax_node();
    let boundaries: Vec<_> = raw
        .into_iter()
        .flat_map(|root| {
            root.children()
                .filter(|n| syntax::preprocess::Event::from_node(n).is_some())
                .map(|n| n.text_range().end())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut regions = vec![Vec::new(); boundaries.len() + 1];
    if let Some(root) = syntax::parse_query::semantic_parse_file(db, file).syntax_node() {
        for (index, node) in root.children().enumerate() {
            let region = boundaries.partition_point(|&end| end <= node.text_range().start());
            regions[region].push(index);
        }
    }
    regions
}

/// Available only for a configured source project. Legacy ad-hoc workspaces do
/// not supply a compiler execution order and must not invent one from FileIds.
pub fn declarations(db: &dyn salsa::Database) -> Option<&Vec<Declaration>> {
    base_db::preprocess::WorkspaceSourceProject::try_get(db)?;
    Some(ordered(db, WorkspaceFiles::try_get(db)?, WorkspacePreprocessing::try_get(db)?))
}

/// Compiler order can be supplied by the workspace project or by one inline
/// program. Different ad-hoc buffers remain separate programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Program {
    Workspace,
    Inline(FileText),
}

pub fn program(db: &dyn salsa::Database, file: FileText) -> Option<Program> {
    match base_db::preprocess::project_membership(db, file.file_id(db)) {
        Some(true) => Some(Program::Workspace),
        // Detached buffers have a local source order, never the project's.
        Some(false) => Some(Program::Inline(file)),
        None => inline::project(db, file).is_some().then_some(Program::Inline(file)),
    }
}

impl Program {
    pub fn declarations(self, db: &dyn salsa::Database) -> &[Declaration] {
        match self {
            Self::Workspace => declarations(db).map_or(&[], Vec::as_slice),
            Self::Inline(file) => inline::declarations(db, file),
        }
    }
}

pub fn declarations_for(db: &dyn salsa::Database, file: FileText) -> Option<&[Declaration]> {
    Some(program(db, file)?.declarations(db))
}

#[salsa::tracked(returns(ref))]
fn ordered(
    db: &dyn salsa::Database,
    files: WorkspaceFiles,
    preprocessing: WorkspacePreprocessing,
) -> Vec<Declaration> {
    let files: HashMap<_, _> = files.file_texts(db).iter().map(|&f| (f.file_id(db), f)).collect();
    let mut result = Vec::new();
    for &(id, region) in preprocessing.order(db) {
        db.unwind_if_revision_cancelled();
        let Some(&file) = files.get(&id) else { continue };
        if let Some(indices) = file_regions(db, file).get(region) {
            result.extend(indices.iter().map(|&index| Declaration { file, index }));
        }
    }
    result
}

pub fn node(db: &dyn salsa::Database, declaration: Declaration) -> Option<syntax::SyntaxNode> {
    syntax::parse_query::semantic_parse_file(db, declaration.file)
        .syntax_node()?
        .children()
        .nth(declaration.index)
}

/// Locate a program point without putting byte offsets into semantic keys.
pub fn at_offset(db: &dyn salsa::Database, file: FileText, offset: usize) -> Option<Declaration> {
    let root = syntax::parse_query::semantic_parse_file(db, file).syntax_node()?;
    root.children().enumerate().find_map(|(index, node)| {
        let range = node.text_range();
        (usize::from(range.start()) <= offset && offset < usize::from(range.end()))
            .then_some(Declaration { file, index })
    })
}

/// A body may retain its old source map after preceding text moves. Resolve
/// its declaration through the current body list, then backdate the ordinal.
#[salsa::tracked]
pub fn for_body<'db>(
    db: &'db dyn salsa::Database,
    id: crate::def_query::DefWithBodyId<'db>,
) -> Option<Declaration> {
    let bodies = crate::def_query::callable_bodies(db, id.file(db)).as_ref()?;
    let body = bodies
        .0
        .entries()
        .iter()
        .filter(|b| b.name == *id.name(db))
        .nth(id.clause_index(db) as usize)?;
    at_offset(db, id.file(db), body.def_span.start)
}
