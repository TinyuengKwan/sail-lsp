//! Coverage of committed deferred obligations. Speculative checks disappear on
//! rollback; immediate subtype proofs are deliberately outside this denominator.
use super::{ConstraintExpr, ObligationCause};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObligationStats {
    pub proved: usize,
    pub refuted: usize,
    pub unknown: usize,
}
impl ObligationStats {
    pub fn total(&self) -> usize {
        self.proved + self.refuted + self.unknown
    }
    pub fn merge(&mut self, other: Self) {
        self.proved += other.proved;
        self.refuted += other.refuted;
        self.unknown += other.unknown;
    }
}

/// Observed blockers, not a claim that adding assumptions necessarily proves a goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownConstraintReason {
    Recovery,
    UnsupportedGoal,
    ErasedNumeric,
    FlexibleWitness,
    BudgetExhausted,
    SolverIncomplete,
    NotEstablished,
}
impl UnknownConstraintReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recovery => "type_recovery",
            Self::UnsupportedGoal => "unsupported_goal",
            Self::ErasedNumeric => "erased_numeric",
            Self::FlexibleWitness => "flexible_witness",
            Self::BudgetExhausted => "budget_exhausted",
            Self::SolverIncomplete => "solver_incomplete",
            Self::NotEstablished => "not_established",
        }
    }
    pub(super) fn classify(
        goal: &ConstraintExpr,
        cause: ObligationCause,
        flexible: bool,
        budget_exhausted: bool,
        incomplete: bool,
    ) -> Self {
        if cause == ObligationCause::Recovery {
            Self::Recovery
        } else if unsupported(goal) {
            Self::UnsupportedGoal
        } else if super::solver::has_erased_numeric(goal) {
            Self::ErasedNumeric
        } else if flexible {
            Self::FlexibleWitness
        } else if budget_exhausted {
            Self::BudgetExhausted
        } else if incomplete {
            Self::SolverIncomplete
        } else {
            Self::NotEstablished
        }
    }
}

pub(super) fn unsupported(goal: &ConstraintExpr) -> bool {
    match goal {
        ConstraintExpr::Unsupported => true,
        ConstraintExpr::And(xs) | ConstraintExpr::Or(xs) => xs.iter().any(unsupported),
        ConstraintExpr::Not(x) => unsupported(x),
        _ => false,
    }
}
