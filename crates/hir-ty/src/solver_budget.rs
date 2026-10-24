//! One cooperative SMT budget shared by every query in an editor request.
//! Nested inference and fallback attempts share both elapsed solver time and
//! check count. Timeout/cancellation is Unknown and is never a proof cache entry.
use crate::CancellationToken;
#[cfg(any(feature = "z3-solver", test))]
use std::time::Instant;
use std::{cell::RefCell, rc::Rc, time::Duration};

#[derive(Clone, Copy, Debug)]
pub struct SolverLimits {
    pub total: Duration,
    pub per_check: Duration,
    pub checks: usize,
}
impl Default for SolverLimits {
    fn default() -> Self {
        Self { total: Duration::from_millis(200), per_check: Duration::from_millis(50), checks: 64 }
    }
}
struct State {
    remaining: Duration,
    #[cfg(any(feature = "z3-solver", test))]
    slice: Duration,
    checks: usize,
    incomplete: usize,
    budget_exhaustions: usize,
    next_scope: usize,
    tokens: Vec<(usize, CancellationToken)>,
}
thread_local! { static ACTIVE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) }; }

/// RAII scope; nested scopes cannot replenish an outer request's budget.
pub struct SolverBudget {
    state: Rc<RefCell<State>>,
    id: usize,
}
impl SolverBudget {
    pub fn enter(token: CancellationToken, limits: SolverLimits) -> Self {
        ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot
                .get_or_insert_with(|| {
                    Rc::new(RefCell::new(State {
                        remaining: limits.total,
                        #[cfg(any(feature = "z3-solver", test))]
                        slice: limits.per_check,
                        checks: limits.checks,
                        incomplete: 0,
                        budget_exhaustions: 0,
                        next_scope: 0,
                        tokens: Vec::new(),
                    }))
                })
                .clone();
            let id = state.borrow().next_scope;
            state.borrow_mut().next_scope += 1;
            state.borrow_mut().tokens.push((id, token));
            Self { state, id }
        })
    }
    pub(crate) fn incomplete_count() -> usize {
        ACTIVE.with(|slot| slot.borrow().as_ref().map_or(0, |state| state.borrow().incomplete))
    }
    #[cfg(feature = "z3-solver")]
    pub(crate) fn mark_incomplete() {
        ACTIVE.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                state.borrow_mut().incomplete += 1;
            }
        });
    }
    pub(crate) fn budget_exhaustion_count() -> usize {
        ACTIVE.with(|slot| slot.borrow().as_ref().map_or(0, |s| s.borrow().budget_exhaustions))
    }
    #[cfg(feature = "z3-solver")]
    pub(crate) fn mark_budget_exhausted() {
        ACTIVE.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                state.borrow_mut().budget_exhaustions += 1;
            }
        });
    }
    pub fn exhausted() -> bool {
        ACTIVE.with(|slot| {
            slot.borrow().as_ref().is_some_and(|state| {
                let s = state.borrow();
                s.remaining.is_zero()
                    || s.checks == 0
                    || s.tokens.iter().any(|(_, token)| token.is_cancelled())
            })
        })
    }
}
impl Drop for SolverBudget {
    fn drop(&mut self) {
        self.state.borrow_mut().tokens.retain(|(id, _)| *id != self.id);
        if self.state.borrow().tokens.is_empty() {
            ACTIVE.with(|slot| {
                let mut active = slot.borrow_mut();
                if active.as_ref().is_some_and(|state| Rc::ptr_eq(state, &self.state)) {
                    *active = None;
                }
            });
        }
    }
}

#[cfg(any(feature = "z3-solver", test))]
pub(crate) struct CheckPermit {
    state: Rc<RefCell<State>>,
    started: Instant,
    pub timeout_ms: u32,
}
#[cfg(any(feature = "z3-solver", test))]
impl CheckPermit {
    pub fn cancellation_tokens(&self) -> Vec<CancellationToken> {
        self.state.borrow().tokens.iter().map(|(_, t)| t.clone()).collect()
    }
    pub fn cancelled(&self) -> bool {
        self.state.borrow().tokens.iter().any(|(_, token)| token.is_cancelled())
    }
}
#[cfg(any(feature = "z3-solver", test))]
impl Drop for CheckPermit {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.remaining = state.remaining.saturating_sub(self.started.elapsed());
    }
}
#[cfg(any(feature = "z3-solver", test))]
pub(crate) fn start_check() -> Option<CheckPermit> {
    ACTIVE.with(|slot| {
        let state = slot.borrow().as_ref()?.clone();
        let mut s = state.borrow_mut();
        if s.checks == 0
            || s.remaining.is_zero()
            || s.tokens.iter().any(|(_, token)| token.is_cancelled())
        {
            return None;
        }
        let timeout = s.remaining.min(s.slice);
        if timeout < Duration::from_millis(1) {
            return None;
        }
        s.checks -= 1;
        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        drop(s);
        Some(CheckPermit { state, started: Instant::now(), timeout_ms })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_fallbacks_share_check_budget_and_restore_on_unwind() {
        let _scope = SolverBudget::enter(
            CancellationToken::never(),
            SolverLimits { checks: 1, ..Default::default() },
        );
        {
            let _nested = SolverBudget::enter(CancellationToken::never(), SolverLimits::default());
            let permit = start_check().unwrap();
            assert!(permit.timeout_ms > 0 && permit.timeout_ms <= 50);
            assert_eq!(permit.cancellation_tokens().len(), 2);
            assert!(!permit.cancelled());
        }
        assert!(start_check().is_none());
        assert!(SolverBudget::exhausted());
    }
    #[test]
    fn cancelled_and_zero_budget_requests_do_not_start_smt() {
        let token = CancellationToken::new();
        let _scope = SolverBudget::enter(token.clone(), SolverLimits::default());
        token.cancel();
        assert!(start_check().is_none());
        drop(_scope);
        let _scope = SolverBudget::enter(
            CancellationToken::never(),
            SolverLimits { total: Duration::ZERO, ..Default::default() },
        );
        assert!(start_check().is_none());
    }
    #[test]
    fn dropping_an_outer_scope_does_not_replenish_a_live_inner_scope() {
        let outer = SolverBudget::enter(
            CancellationToken::never(),
            SolverLimits { checks: 0, ..Default::default() },
        );
        let inner = SolverBudget::enter(CancellationToken::never(), SolverLimits::default());
        drop(outer);
        let another = SolverBudget::enter(CancellationToken::never(), SolverLimits::default());
        assert!(start_check().is_none());
        drop(inner);
        drop(another);
        let _fresh = SolverBudget::enter(CancellationToken::never(), SolverLimits::default());
        assert!(start_check().is_some());
    }
    #[test]
    fn panic_restores_request_scope() {
        let _ = std::panic::catch_unwind(|| {
            let _scope = SolverBudget::enter(
                CancellationToken::never(),
                SolverLimits { checks: 0, ..Default::default() },
            );
            panic!("cancelled inference");
        });
        let _scope = SolverBudget::enter(CancellationToken::never(), SolverLimits::default());
        assert!(start_check().is_some());
    }
}
