//! Optional, scoped checkpoints for long computations in database-independent
//! libraries. A parser need not depend on Salsa to cooperate with its caller.
use std::cell::Cell;

thread_local! {
    static CHECK: Cell<Option<fn()>> = const { Cell::new(None) };
}

#[must_use]
pub struct Scope(Option<fn()>);

/// Cancellation of an isolated computation without a shared query consumer.
/// Use `resume_unwind` to propagate it without invoking a panic hook.
#[derive(Debug)]
pub struct Cancelled;

/// Nested scopes restore their caller, including while unwinding.
pub fn enter(check: fn()) -> Scope {
    Scope(CHECK.with(|slot| slot.replace(Some(check))))
}

impl Drop for Scope {
    fn drop(&mut self) {
        CHECK.with(|slot| slot.set(self.0));
    }
}

#[inline]
pub fn checkpoint() {
    if let Some(check) = CHECK.with(Cell::get) {
        check();
    }
}
