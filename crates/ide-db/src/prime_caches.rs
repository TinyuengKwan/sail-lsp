//! Cache pre-warming — trigger salsa queries before first user request.
//!
//! `parallel_prime_caches(db, num_threads, cb)` runs workers in
//! dependency order using rayon for parallel symbol/navigation indexing.

use crate::root_database::RootDatabase;

#[cfg(test)]
mod tests {
    #[test]
    fn priming_does_not_displace_the_active_bodys_inference_cache() {
        use salsa::Database;
        use std::sync::Arc;
        let mut db = crate::root_database::RootDatabase::default();
        let mut change = crate::apply_change::Change::new();
        let ids: Vec<_> = (0..3).map(base_db::FileId::from_raw).collect();
        for (index, &id) in ids.iter().enumerate() {
            change.change_file(id, format!("function f{index}() -> int = {index}\n"));
        }
        change.set_workspace_files(ids.clone());
        db.apply_change(change);
        crate::cache::QueryCacheCapacities { inference: 1, ..Default::default() }.apply(&mut db);
        let file = db.files().file_text(ids[0]).unwrap();
        let body = hir::def_query::file_def_with_body_ids(&db, file)[0];
        let original = Arc::downgrade(&hir::ty_query::infer(&db, body).0);
        db.trigger_lru_eviction();
        assert!(original.upgrade().is_some());
        super::parallel_prime_caches(&db, db.files(), 2, &|_| {}, &hir::CancellationToken::new());
        db.trigger_lru_eviction();
        assert!(original.upgrade().is_some(), "priming evicted the active inferred body");
        let symbols = crate::db_query::file_symbols(&db, db.files().file_text(ids[2]).unwrap());
        assert!(symbols.entries.iter().any(|entry| entry.name == "f2"));

        // Index warm-up still leaves real semantic checks available on demand.
        let mut change = crate::apply_change::Change::new();
        change.change_file(ids[2], "function f2() -> int = true\n");
        db.apply_change(change);
        let file = db.files().file_text(ids[2]).unwrap();
        let body = hir::def_query::file_def_with_body_ids(&db, file)[0];
        assert!(!hir::ty_query::infer(&db, body).0.type_mismatches.is_empty());
    }

    #[test]
    fn priming_leaves_compatibility_views_on_demand() {
        use std::sync::{Arc, Mutex};
        let queries = Arc::new(Mutex::new(Vec::new()));
        let observed = queries.clone();
        let mut db =
            crate::root_database::RootDatabase::with_event_callback(Some(Box::new(move |event| {
                if let salsa::EventKind::WillExecute { database_key } = event.kind {
                    observed.lock().unwrap().push(format!("{database_key:?}"));
                }
            })));
        let id = base_db::FileId::from_raw(0);
        let mut change = crate::apply_change::Change::new();
        change.change_file(id, "function f(x : int) -> int = x\n");
        change.set_workspace_files(vec![id]);
        db.apply_change(change);
        queries.lock().unwrap().clear();
        super::parallel_prime_caches(&db, db.files(), 1, &|_| {}, &hir::CancellationToken::new());
        let queries = queries.lock().unwrap();
        assert!(queries.iter().any(|q| q.starts_with("file_symbols(")), "{queries:?}");
        assert!(queries.iter().all(|q| !q.starts_with("parsed_file(")), "{queries:?}");
    }

    #[test]
    fn primes_only_active_files_even_with_retained_buffers() {
        let mut db = crate::root_database::RootDatabase::default();
        let mut change = crate::apply_change::Change::new();
        for raw in 0..3 {
            change.change_file(base_db::FileId::from_raw(raw), format!("val f{raw} : int\n"));
        }
        change.set_workspace_files(vec![base_db::FileId::from_raw(1)]);
        db.apply_change(change);
        let reports = std::sync::Mutex::new(Vec::new());
        super::parallel_prime_caches(
            &db,
            db.files(),
            1,
            &|p| reports.lock().unwrap().push(p),
            &hir::CancellationToken::new(),
        );
        let reports = reports.into_inner().unwrap();
        assert!(!reports.is_empty());
        assert!(reports.iter().all(|p| p.files_total == 1 && p.files_done <= 1));
    }
}

/// Progress report for cache priming.
#[derive(Debug, Clone)]
pub struct ParallelPrimeCachesProgress {
    /// File names currently being indexed.
    pub files_currently_indexing: Vec<String>,
    /// Total number of files.
    pub files_total: usize,
    /// Number of files processed so far.
    pub files_done: usize,
    /// Current work phase.
    pub work_type: &'static str,
}

/// Pre-warm salsa caches for all files in the workspace.
/// Phase 1 (sequential): Parse + ItemTree per file.
/// Phase 2 (parallel): Per-file symbol and navigation indexes.
/// Body inference stays on demand: checking an entire large model here can
/// take minutes, compete for the solver and displace the active body's cache.
///
/// Takes `&RootDatabase` (concrete type) instead of `&dyn Database`
/// so rayon workers can call `db.clone()`.
pub fn parallel_prime_caches(
    db: &RootDatabase,
    files: &base_db::Files,
    num_worker_threads: usize,
    cb: &(dyn Fn(ParallelPrimeCachesProgress) + Sync),
    cancel: &hir::CancellationToken,
) {
    let mut _sw = profile::StopWatch::start();

    let file_texts: Vec<_> = match base_db::WorkspaceFiles::try_get(db) {
        Some(workspace) => workspace.file_texts(db).clone(),
        None => files.all_file_ids().into_iter().filter_map(|fid| files.file_text(fid)).collect(),
    };
    let total = file_texts.len();
    if total == 0 {
        return;
    }

    let ft_count = file_texts.len();

    // Phase 1: Sequential parse + ItemTree.
    //
    // These queries form per-file dependency chains (parse → ItemTree →
    // Bodies), so sequential per-file is optimal for cache locality.
    for (i, ft) in file_texts.iter().enumerate() {
        if cancel.is_cancelled() {
            log::info!("prime_caches: cancelled at parse phase {}/{}", i, ft_count);
            return;
        }

        let _ = syntax::parse_query::parse_file(db, *ft);
        let _ = hir::def_query::file_item_tree(db, *ft);

        let n = i + 1;
        if n % 10 == 0 || n == ft_count {
            cb(ParallelPrimeCachesProgress {
                files_currently_indexing: Vec::new(),
                files_total: total,
                files_done: n,
                work_type: "Indexing",
            });
        }
    }

    // Phase 2: compact, reusable symbol/navigation indexes, with no SMT work.
    cb(ParallelPrimeCachesProgress {
        files_currently_indexing: Vec::new(),
        files_total: total,
        files_done: 0,
        work_type: "Collecting symbols",
    });

    let thread_count = num_worker_threads.max(1);

    if thread_count <= 1 || cancel.is_cancelled() {
        // Sequential fallback (single thread or already cancelled)
        for (i, &file) in file_texts.iter().enumerate() {
            if cancel.is_cancelled() {
                return;
            }
            let _ = crate::db_query::file_symbols(db, file);
            if (i + 1) % 10 == 0 || i + 1 == total {
                cb(ParallelPrimeCachesProgress {
                    files_currently_indexing: Vec::new(),
                    files_total: total,
                    files_done: i + 1,
                    work_type: "Collecting symbols",
                });
            }
        }
    } else {
        // Each worker owns a cheap Salsa snapshot. File symbols also warm
        // structural call graphs, without inferring the called functions.
        // Each rayon worker gets its own mutable Database via map_with.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build()
            .expect("failed to build rayon thread pool");

        let db_clone = db.clone();
        let cancel_clone = cancel.clone();
        pool.install(move || {
            use rayon::prelude::*;
            file_texts
                .par_iter()
                .map_with(db_clone, |snap, &file| {
                    if !cancel_clone.is_cancelled() {
                        let _ = crate::db_query::file_symbols(snap, file);
                    }
                })
                .count();
        });
    }

    cb(ParallelPrimeCachesProgress {
        files_currently_indexing: Vec::new(),
        files_total: total,
        files_done: total,
        work_type: "Collecting symbols",
    });

    log::info!("prime_caches: {} files ({} threads) in {}", total, thread_count, _sw.elapsed());
}
