//! Per-query LRU controls. Changing capacities requires exclusive database
//! access and belongs at an idle boundary, never in a request snapshot.
use crate::root_database::RootDatabase;

/// Zero disables eviction for that query family. Defaults preserve the
/// existing policy until a measured workload or explicit memory budget opts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryCacheCapacities {
    pub parse: usize,
    pub environment: usize,
    pub inference: usize,
    pub bodies: usize,
}

impl Default for QueryCacheCapacities {
    fn default() -> Self {
        Self { parse: 0, environment: 0, inference: 0, bodies: 512 }
    }
}

impl QueryCacheCapacities {
    /// A pressure policy bounds previously unbounded families and reduces
    /// explicit limits without ever increasing a user's smaller limit.
    pub fn under_pressure(self) -> Self {
        fn reduced(value: usize) -> usize {
            if value == 0 {
                32
            } else {
                (value / 4).max(1)
            }
        }
        Self {
            parse: reduced(self.parse),
            environment: reduced(self.environment),
            inference: reduced(self.inference),
            bodies: reduced(self.bodies),
        }
    }

    pub fn apply(self, db: &mut RootDatabase) {
        syntax::parse_query::set_parse_cache_capacity(db, self.parse);
        hir::ty_query::set_inference_cache_capacities(db, self.environment, self.inference);
        hir::def_query::set_body_cache_capacity(db, self.bodies);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply_change::Change;
    use base_db::FileId;
    use salsa::Database;
    use std::sync::Arc;

    #[test]
    fn capacity_updates_reclaim_results_and_recomputed_types_stay_fresh() {
        let mut db = RootDatabase::default();
        let mut change = Change::new();
        for n in 0..24 {
            change.change_file(FileId::from_raw(n), format!("function f{n}() -> int = {n}\n"));
        }
        db.apply_change(change);
        let visit = |db: &RootDatabase| {
            (0..24)
                .map(|n| {
                    let file = db.files().file_text(FileId::from_raw(n)).unwrap();
                    let id = hir::def_query::file_def_with_body_ids(db, file)[0];
                    let result = hir::ty_query::infer(db, id);
                    assert!(result.0.type_mismatches.is_empty());
                    Arc::downgrade(&result.0)
                })
                .collect::<Vec<_>>()
        };
        let original = visit(&db);
        db.trigger_lru_eviction();
        assert_eq!(original.iter().filter(|weak| weak.strong_count() != 0).count(), 24);
        QueryCacheCapacities { inference: 4, ..Default::default() }.apply(&mut db);
        db.trigger_lru_eviction();
        assert_eq!(original.iter().filter(|weak| weak.strong_count() != 0).count(), 4);
        let after = visit(&db);
        assert!(after[0].upgrade().is_some(), "evicted result must recompute successfully");
        QueryCacheCapacities::default().apply(&mut db);
        db.trigger_lru_eviction();
        assert!(
            after.iter().all(|weak| weak.strong_count() != 0),
            "zero restores unbounded retention"
        );
        let mut change = Change::new();
        change.change_file(FileId::from_raw(0), "function f0() -> int = true\n");
        db.apply_change(change);
        let file = db.files().file_text(FileId::from_raw(0)).unwrap();
        let id = hir::def_query::file_def_with_body_ids(&db, file)[0];
        assert!(
            !hir::ty_query::infer(&db, id).0.type_mismatches.is_empty(),
            "retuning must not mask a later type error"
        );
    }
}
