//! A thin wrapper around rayon which threads a sender through spawned jobs.
//! `TaskPool<T>` is generic — it doesn't know about the `Task` enum.
//! The `Task` enum is defined in `main_loop.rs`.
//!
//! `catch_unwind` at the worker thread level. We use
//! rayon, which aborts on panic. We wrap every spawn closure in
//! `catch_unwind` for resilience.

use crossbeam_channel::Sender;

/// Rayon-backed thread pool with panic resilience.
pub(crate) struct TaskPool<T> {
    sender: Sender<T>,
    pool: rayon::ThreadPool,
}

impl<T: Send + 'static> TaskPool<T> {
    /// Create a new task pool with the given sender and thread count.
    pub(crate) fn new_with_threads(sender: Sender<T>, threads: usize) -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|ix| format!("sail-worker-{ix}"))
            .build()
            .expect("failed to create rayon thread pool");
        Self { sender, pool }
    }

    /// Spawn a background task on the rayon pool.
    /// Wrapped in `catch_unwind` for panic resilience.
    pub(crate) fn spawn<F>(&self, task: F)
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let sender = self.sender.clone();
        self.pool.spawn(move || {
            if let Ok(result) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task())) {
                let _ = sender.send(result);
            }
        });
    }

    /// Spawn a task that can send multiple results back.
    /// Wrapped in `catch_unwind` for panic resilience.
    pub(crate) fn spawn_with_sender<F>(&self, task: F)
    where
        F: FnOnce(Sender<T>) + Send + 'static,
    {
        let sender = self.sender.clone();
        self.pool.spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(sender)));
        });
    }
}
