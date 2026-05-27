//! `hir_expand` handles $include expansion for the Sail language.
//!
//! in the dependency DAG: `syntax -> hir-expand -> hir-def -> hir-ty`.
//!
//! In RA, `hir-expand` handles macro expansion (declarative, procedural, builtin
//! derives) and produces expanded syntax trees. In Sail, it handles `$include`
//! graph resolution and determines the file set that hir-def should process.
//!
//! ## Key differences from RA
//!
//! - No macro expansion (Sail has no macros)
//! - `$include` graph replaces macro file tracking
//! - `AnalysisScope` replaces hygiene (determines visible file set)
//! - No `MacroFile` / `MacroCallId` -- all files are "real" files
//!
//! ## Intentional gaps (no Sail equivalent)
//!
//! - `tt` crate: Sail has no token trees
//! - `mbe` crate: Sail has no macro-by-example
//! - `syntax-bridge`: no CST<->TT conversion needed
//! - `cfg` crate: no conditional compilation (closest: .sail_project visibility)
//! - `edition` crate: Sail has no editions
//! - `proc-macro-*`: no procedural macros
//! - `HygieneFrame`: $include has no hygiene concerns

pub mod analysis_scope;
pub mod db;
pub mod in_file;
pub mod include_graph;
pub mod query;

pub use analysis_scope::AnalysisScope;
pub use db::ExpandDatabase;
pub use in_file::{FilePosition, FileRange, InFile};
pub use include_graph::IncludeGraph;
pub use query::{
    include_paths, typed_include_paths, ArcIncludeGraph, IncludePath, WorkspaceIncludeGraph,
};
