//! Cache pre-warming — trigger salsa queries before first user request.
//!
//! `parallel_prime_caches(db, num_threads, cb)` runs workers in
//! dependency order using rayon for parallel inference.

use crate::root_database::RootDatabase;

#[cfg(test)]
mod tests {
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
/// Phase 1 (sequential): Parse + ItemTree + Bodies per file.
/// Phase 2 (parallel): Type inference for all callables via rayon.
///
/// ```text
/// bodies.par_iter().map_with(db.clone(), |snap, &body| {
///     InferenceResult::of(snap, body);
/// }).count();
/// ```
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

    // Phase 1: Sequential parse + ItemTree + Bodies (fast: ~230ms).
    //
    // These queries form per-file dependency chains (parse → ItemTree →
    // Bodies), so sequential per-file is optimal for cache locality.
    for (i, ft) in file_texts.iter().enumerate() {
        if cancel.is_cancelled() {
            log::info!("prime_caches: cancelled at parse phase {}/{}", i, ft_count);
            return;
        }

        let _ = syntax::parse_query::parse_file(db, *ft);
        let _ = syntax::parse_query::parsed_file(db, *ft);
        let _ = hir::def_query::file_item_tree(db, *ft);
        let _ = hir::def_query::callable_bodies(db, *ft);

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

    // Phase 2: Parallel inference via rayon.
    //
    //   bodies.par_iter()
    //       .map_with(db.clone(), |snap, &body| {
    //           InferenceResult::of(snap, body);
    //       })
    //       .count();
    //
    // Each rayon worker gets its own db clone (cheap: salsa Arc).
    let all_callable_ids: Vec<hir::def_query::DefWithBodyId> = file_texts
        .iter()
        .flat_map(|ft| hir::def_query::file_def_with_body_ids(db, *ft).to_vec())
        .collect();

    let total_callables = all_callable_ids.len();
    if total_callables == 0 {
        log::info!("prime_caches: {} files, 0 callables in {}", total, _sw.elapsed());
        return;
    }

    cb(ParallelPrimeCachesProgress {
        files_currently_indexing: Vec::new(),
        files_total: total_callables,
        files_done: 0,
        work_type: "Type inference",
    });

    let thread_count = num_worker_threads.max(1);

    if thread_count <= 1 || cancel.is_cancelled() {
        // Sequential fallback (single thread or already cancelled)
        for (i, &id) in all_callable_ids.iter().enumerate() {
            if cancel.is_cancelled() {
                return;
            }
            let _ = hir::ty_query::infer(db, id);
            if (i + 1) % 200 == 0 || i + 1 == total_callables {
                cb(ParallelPrimeCachesProgress {
                    files_currently_indexing: Vec::new(),
                    files_total: total_callables,
                    files_done: i + 1,
                    work_type: "Type inference",
                });
            }
        }
    } else {
        // Parallel inference with rayon.
        //   bodies.par_iter()
        //       .map_with(db.clone(), |snap, &body| {
        //           InferenceResult::of(snap, body);
        //       }).count();
        //
        // db.clone() creates a cheap salsa snapshot (Arc-backed).
        // Each rayon worker gets its own mutable Database via map_with.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build()
            .expect("failed to build rayon thread pool");

        let db_clone = db.clone();
        let cancel_clone = cancel.clone();
        pool.install(move || {
            use rayon::prelude::*;
            all_callable_ids
                .par_iter()
                .map_with(db_clone, |snap, &id| {
                    if !cancel_clone.is_cancelled() {
                        let _ = hir::ty_query::infer(snap, id);
                    }
                })
                .count();
        });
    }

    cb(ParallelPrimeCachesProgress {
        files_currently_indexing: Vec::new(),
        files_total: total_callables,
        files_done: total_callables,
        work_type: "Type inference",
    });

    log::info!(
        "prime_caches: {} files, {} callables ({} threads) in {}",
        total,
        total_callables,
        thread_count,
        _sw.elapsed()
    );
}

/// Legacy entry point — wraps `parallel_prime_caches` for backward compat.
///
/// New code should call `parallel_prime_caches` directly.
pub fn prime_caches(
    db: &RootDatabase,
    files: &base_db::Files,
    on_progress: &(dyn Fn(ParallelPrimeCachesProgress) + Sync),
) {
    let cancel = hir::CancellationToken::new();
    parallel_prime_caches(db, files, 1, on_progress, &cancel);
}
