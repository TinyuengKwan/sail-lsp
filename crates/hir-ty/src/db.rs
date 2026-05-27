//! Type-inference database trait.
//!
//! tracked queries for the type-checking layer.
//!
//! Now properly extends DefDatabase (cycle broken in -).
//!
//!   SourceDatabase  (base-db)
//!        ↓
//!   DefDatabase     (hir-def)
//!        ↓
//!   HirDatabase     (hir-ty)  ← this trait
//!        ↓
//!   RootDatabase    (ide-db)  ← concrete struct

use base_db::FileText;
use hir_def::db::DefDatabase;
use hir_def::def_query::DefWithBodyId;

use crate::query::{self, ArcInferenceResult, ArcTopLevelEnv};

/// Database trait for type-inference queries.
///
/// Queries (5 total vs RA's ~40+):
///   - `top_level_env`       —
///   - `infer_body`          —
///   - `infer`               —
///   - `infer_for_body`      —
///   - `transitive_effects`  —
pub trait HirDatabase: DefDatabase {
    /// for body-only-edit invalidation (no RA counterpart).
    fn top_level_env(&self, input: FileText) -> &ArcTopLevelEnv;
    fn infer_body(&self, input: FileText) -> &ArcInferenceResult;
    fn infer<'db>(&'db self, id: DefWithBodyId<'db>) -> &'db ArcInferenceResult;
    fn infer_for_body<'db>(&'db self, id: DefWithBodyId<'db>) -> &'db ArcInferenceResult;
    fn transitive_effects(&self, input: FileText) -> &query::ArcTransitiveEffects;
}
