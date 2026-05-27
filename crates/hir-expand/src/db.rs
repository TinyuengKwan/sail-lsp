//! Expand-layer database trait.
//!
//! queries that sit between base-db and hir-def in the trait hierarchy:
//!
//!   SourceDatabase  (base-db)
//!        |
//!   ExpandDatabase  (hir-expand)  <-- this trait
//!        |
//!   DefDatabase     (hir-def)
//!        |
//!   HirDatabase     (hir-ty)
//!        |
//!   RootDatabase    (ide-db)

use base_db::{FileText, SourceDatabase};

/// Database trait for expand-layer queries.
pub trait ExpandDatabase: SourceDatabase {
    fn include_paths(&self, input: FileText) -> &[String];
}
