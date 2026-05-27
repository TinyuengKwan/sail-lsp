//! Definition-layer database trait.
//!
//! per-file definition-level data: ItemTree, CallableBodies, DefMap, CallGraph,
//! DefWithBodyId interning.
//!
//! Higher-level crates (`hir-ty`, `ide-db`) extend this with their
//! own database traits, forming the layered hierarchy:
//!
//!   SourceDatabase  (base-db)
//!        ↓
//!   ExpandDatabase  (hir-expand)
//!        ↓
//!   DefDatabase     (hir-def)  ← this trait
//!        ↓
//!   HirDatabase     (hir-ty)
//!        ↓
//!   RootDatabase    (ide-db)   ← concrete struct

use std::sync::Arc;

use base_db::FileText;
use hir_expand::db::ExpandDatabase;

use crate::bodies::CallableBodies;
use crate::callgraph::CallGraph;
use crate::def_query::{ArcBodyWithSourceMap, DefWithBodyId};
use crate::item_tree::ItemTree;
use crate::nameres::DefMap;

/// Database trait for definition-layer queries.
///
/// Inherits `include_paths` from ExpandDatabase.
/// Dyn-safe (no `Sized` bound).
pub trait DefDatabase: ExpandDatabase {
    fn file_item_tree(&self, input: FileText) -> Option<&Arc<ItemTree>>;
    fn callable_bodies(&self, input: FileText) -> Option<&CallableBodies>;
    fn def_map(&self, input: FileText) -> Option<&DefMap>;
    fn callgraph(&self, input: FileText) -> Option<&CallGraph>;
    fn file_def_with_body_ids<'db>(&'db self, input: FileText) -> &'db [DefWithBodyId<'db>];
    fn body_with_source_map<'db>(&'db self, id: DefWithBodyId<'db>) -> &'db ArcBodyWithSourceMap;
}
