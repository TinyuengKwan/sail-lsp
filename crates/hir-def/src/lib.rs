//! Definition-side HIR layers: body arenas, item tree, DefMap, Resolver.
pub use hir_expand::analysis_scope;
pub use parser::{Span, Token};
pub mod ast_id;
/// Bitfield accessor materialization.
pub mod bitfield;
pub mod bodies;
pub mod callable_info;
pub mod callgraph;
pub mod db;
pub mod def_query;
pub mod diagnostics;
pub mod effects;
pub mod expr_store;
/// Path finding for auto-import ($include).
pub mod find_path;
pub mod hir;
pub use hir_expand::in_file;
pub use hir_expand::include_graph;
pub mod item_id;
pub mod item_scope;
pub mod item_tree;
pub mod message;
pub mod name;
pub mod nameres;
pub mod per_ns;
pub use project_model as project;
pub mod resolver;
pub mod scattered;
pub mod signatures;
pub mod src;
pub mod type_error;
/// Re-export for backward compatibility.
pub use hir::type_ref;
#[cfg(test)]
mod test_db;
pub mod visibility;
pub mod workspace_def_map;

pub use bodies::{CallableBodies, CallableBody, EffectTag};
pub use db::DefDatabase;
pub use expr_store::body::{expr_id_from_raw, pat_id_from_raw, Body, BodySourceMap};
pub use expr_store::hir::{
    Expr, ExprId, ExprOrPatId, MappingArm, MappingDirection, MatchArm, Pat, PatId, Statement,
};
pub use expr_store::{Binding, BindingId};
/// Backward-compat: `crate::body` re-exports from `crate::expr_store::body`.
pub mod body {
    pub use crate::expr_store::body::*;
}
pub use item_scope::ItemScope;
pub use item_tree::{Associativity, FixityDecl, ItemKind, ItemTree, ItemTreeEntry};
pub use name::Name;
pub use nameres::{DefData, DefDiagnostic, DefId, DefMap, ModuleData, ModuleId, ModuleOrigin};
pub use per_ns::{Namespace, PerNs};
pub use project::{parse_project, ProjectFile, ProjectParseError};
pub use resolver::{Resolution, ResolveValueResult, Resolver, TypeNs, ValueNs};
pub use scattered::{
    workspace_scattered_status, ScatteredHeadLocation, ScatteredKind, ScatteredStatus,
};
pub use visibility::{RawVisibility, Visibility};
pub use workspace_def_map::{GlobalDef, WorkspaceDefMap};

pub use item_id::{
    DefWithBodyId, FunctionId, FunctionLoc, LetId, LetLoc, MappingId, MappingLoc, ModuleDefId,
    OverloadId, OverloadLoc, RegisterId, RegisterLoc, TypeDefId, TypeDefLoc, ValSpecId, ValSpecLoc,
};

pub use ast_id::{AstId, AstIdMap, ErasedAstId, ErasedFileAstId, FileAstId};
pub use in_file::{FilePosition, FileRange, InFile};
pub use src::{def_id_source, def_source, HasSource};
