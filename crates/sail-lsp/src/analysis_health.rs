//! Bounded, passive observations of analysis failures and slow requests.
//! Cancellation is expected control flow; an undecided proof is not a panic.
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    pub generation: usize,
    pub message: String,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlowRequest {
    pub method: String,
    pub duration_ms: u64,
    pub error_code: Option<i32>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisHealth {
    pub failures: BTreeMap<String, Failure>,
    pub cancelled_waves: u64,
    /// Reasons on the latest published semantic wave, not lifetime solver totals.
    pub reported_unknowns: BTreeMap<String, usize>,
    pub recent_requests: VecDeque<SlowRequest>,
}

impl AnalysisHealth {
    pub fn failed(&mut self, phase: &str, generation: usize, message: String) {
        self.failures.insert(phase.to_string(), Failure { generation, message });
    }

    pub fn recovered(&mut self, phase: &str) {
        self.failures.remove(phase);
    }

    pub fn record_request(&mut self, method: String, duration_ms: u64, error_code: Option<i32>) {
        const CAPACITY: usize = 64;
        if self.recent_requests.len() == CAPACITY {
            self.recent_requests.pop_front();
        }
        self.recent_requests.push_back(SlowRequest { method, duration_ms, error_code });
    }
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
        .chars()
        .take(2048)
        .collect()
}
