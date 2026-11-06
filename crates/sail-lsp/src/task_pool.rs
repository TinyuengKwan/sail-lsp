//! The pool background work runs on, and what it is running for.
//!
//! `TaskPool<T>` is generic — it doesn't know about the `Task` enum, which
//! is defined in `main_loop.rs`.
//!
//! Backed by `stdx::thread::Pool`, rust-analyzer's, because a job says what
//! it is *for*: a completion is caused by the user typing and a workspace
//! scan is not, and the pool sets the thread's QoS class accordingly. This
//! was rayon, which has no such notion, and `stdx::thread` — vendored,
//! complete, `ThreadIntent` and all — had one caller in the whole
//! workspace (`vfs-notify`).
//!
//! **On Linux and Windows the intent is currently a no-op**: the QoS facade
//! is implemented for Apple platforms and stubbed elsewhere, with RA's own
//! FIXME on the Windows case. It is wired anyway because the classification
//! is the part that has to be right — which request is in the critical path
//! of typing is a fact about this server, not about the kernel — and
//! because the pool it comes with counts its extant tasks, which is what
//! the main loop asks before evicting.

use crossbeam_channel::Sender;
use stdx::thread::{Pool, ThreadIntent};

pub(crate) struct TaskPool<T> {
    sender: Sender<T>,
    pool: Pool,
}

impl<T: Send + 'static> TaskPool<T> {
    pub(crate) fn sender(&self) -> Sender<T> {
        self.sender.clone()
    }

    /// Create a new task pool with the given sender and thread count.
    pub(crate) fn new_with_threads(sender: Sender<T>, threads: usize) -> Self {
        Self { sender, pool: Pool::new(threads.max(1)) }
    }

    /// How many threads a machine should give this pool.
    ///
    /// Bound the automatic value because each solver-owning worker can retain
    /// a native thread-local Z3 context. The eight-worker ceiling preserves
    /// request parallelism without scaling context retention to every core.
    pub(crate) fn default_threads() -> usize {
        std::thread::available_parallelism().map_or(2, usize::from).clamp(2, 8)
    }

    /// Spawn a background task, saying what it is for.
    ///
    /// Panics are caught: rayon aborted the process on one, and this pool
    /// re-raises on its own thread, so the server would die with it.
    pub(crate) fn spawn<F>(&self, intent: ThreadIntent, task: F)
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let sender = self.sender.clone();
        // `AssertUnwindSafe` at the door rather than in every caller's
        // signature: a handler's closure captures an `Analysis` snapshot,
        // which is full of interior mutability and never `UnwindSafe`, and
        // the panic is caught here anyway.
        self.pool.spawn(
            intent,
            std::panic::AssertUnwindSafe(move || {
                if let Ok(result) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)) {
                    let _ = sender.send(result);
                }
            }),
        );
    }

    /// Spawn a task that can send multiple results back.
    pub(crate) fn spawn_with_sender<F>(&self, intent: ThreadIntent, task: F)
    where
        F: FnOnce(Sender<T>) + Send + 'static,
    {
        let sender = self.sender.clone();
        self.pool.spawn(
            intent,
            std::panic::AssertUnwindSafe(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(sender)));
            }),
        );
    }

    /// Is nothing running?
    ///
    /// Read before evicting: a task that is still working holds an
    /// `Analysis` snapshot, and eviction would cancel it. The count is the
    /// pool's own `extant_tasks`, which is what rust-analyzer's main loop
    /// asks the same question of.
    pub(crate) fn is_empty(&self) -> bool {
        self.pool.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskPool, ThreadIntent};

    /// A task in flight is counted, and a panicking one still finishes.
    #[test]
    fn the_pool_says_whether_anything_is_running() {
        let (sender, receiver) = crossbeam_channel::unbounded::<u32>();
        let pool = TaskPool::new_with_threads(sender, 1);
        assert!(pool.is_empty(), "a fresh pool is idle");

        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        pool.spawn(ThreadIntent::Worker, move || {
            let _ = gate_rx.recv();
            7
        });
        // The task is either queued or running; either way it is extant.
        assert!(!pool.is_empty(), "a spawned task is counted");
        let _ = gate_tx.send(());
        assert_eq!(receiver.recv().expect("the task answers"), 7);

        pool.spawn(ThreadIntent::Worker, || panic!("the worker gives up"));
        // Yielding a fixed number of times does not guarantee the worker
        // gets scheduled, especially while other test binaries are busy.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pool.is_empty() {
            assert!(std::time::Instant::now() < deadline, "a panicking task is still counted out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(pool.is_empty(), "a panicking task is still counted out");
    }

    /// A machine with cores gets parallelism without unbounded native solver owners.
    #[test]
    fn the_pool_bounds_automatic_parallelism() {
        assert!((2..=8).contains(&TaskPool::<u32>::default_threads()));
    }
}
