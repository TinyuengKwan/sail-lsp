//! Cooperative cancellation token threaded through the type checker.
//!
//! Stage — extracted from `sail_server::typecheck` so anything
//! that wants to schedule a typecheck (sail_server's debounced worker
//! today, future hir-ty consumers tomorrow) doesn't need to import
//! the 10k-line typecheck module just to get a cancellation primitive.
//!
//! The worker that schedules a typecheck can flip this to `true` when
//! a newer typecheck is queued for the same file; the in-flight
//! checker notices at the next per-definition checkpoint and bails
//! out early instead of burning CPU on a result that will be
//! discarded.
//!
//! This is a much weaker analogue of salsa's `Cancelled` exception,
//! but it's enough to keep typing latency from piling up doomed work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self { inner: Arc::new(AtomicBool::new(false)) }
    }

    /// Returns a token that can never be cancelled. Tests and one-shot
    /// callers use this so they don't have to thread a real token.
    pub fn never() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }
}
