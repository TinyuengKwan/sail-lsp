//! Idle-only query collection with an optional process-memory high-water mark.
//! A soft target cannot reclaim inputs, client snapshots or arbitrary native
//! allocations. Keep normal cancellation and solver budgets independent.
use crate::{config::CacheConfig, global_state::GlobalState};
use ide_db::cache::QueryCacheCapacities;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub(crate) struct CacheController {
    config: CacheConfig,
    under_pressure: bool,
    last_sample: Option<Instant>,
    last_rss: Option<u64>,
}

impl CacheController {
    fn update(
        &mut self,
        config: &CacheConfig,
        now: Instant,
        rss: Option<u64>,
    ) -> Option<QueryCacheCapacities> {
        let changed = self.config != *config;
        if !changed
            && self
                .last_sample
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(5))
        {
            return None;
        }
        self.last_sample = Some(now);
        self.last_rss = rss;
        let previous = self.under_pressure;
        if config.memory_limit_mb == 0 {
            self.under_pressure = false;
        } else if let Some(rss) = rss {
            let limit = u64::from(config.memory_limit_mb) * 1024 * 1024;
            if rss >= limit {
                self.under_pressure = true;
            } else if rss < limit * 4 / 5 {
                self.under_pressure = false;
            }
        }
        let capacities = config.capacities();
        let previous_capacities = self.config.capacities();
        self.config = config.clone();
        // Repeat eviction at most once per sample while above the target;
        // repeated requests never repeatedly halve already reduced limits.
        (capacities != previous_capacities
            || previous != self.under_pressure
            || self.under_pressure)
            .then(|| if self.under_pressure { capacities.under_pressure() } else { capacities })
    }

    pub(crate) fn report(&self) -> serde_json::Value {
        let capacities = if self.under_pressure {
            self.config.capacities().under_pressure()
        } else {
            self.config.capacities()
        };
        serde_json::json!({"underPressure":self.under_pressure,"softMemoryLimitMiB":self.config.memory_limit_mb,
            "lastSampledRssBytes":self.last_rss,"effectiveCapacities":{
                "parse":capacities.parse,"environment":capacities.environment,
                "inference":capacities.inference,"bodies":capacities.bodies}})
    }
}

impl GlobalState {
    /// Returns true only when collection actually ran. Snapshot ownership is
    /// checked in addition to worker idleness: a retained snapshot must never
    /// deadlock a capacity update or get cancelled merely to save memory.
    pub(crate) fn collect_caches_if_idle(&mut self) -> bool {
        if !self.task_pool.is_empty() || self.analysis_host.raw_database().live_snapshots() != 0 {
            return false;
        }
        let now = Instant::now();
        let needs_sample = self.cache_control.config != self.config.cache
            || self
                .cache_control
                .last_sample
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5));
        let policy = if needs_sample {
            let rss = (self.config.cache.memory_limit_mb != 0)
                .then(profile::process_resident_bytes)
                .flatten();
            self.cache_control.update(&self.config.cache, now, rss)
        } else {
            None
        };
        let nonce = self.analysis_host.raw_database().nonce();
        if policy.is_none() && nonce == self.last_gc_revision {
            return false;
        }
        if let Some(capacities) = policy {
            capacities.apply(self.analysis_host.raw_database_mut());
        }
        self.analysis_host.trigger_garbage_collection();
        if self.cache_control.under_pressure {
            profile::release_unused_memory();
        }
        self.last_gc_revision = nonce;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_uses_hysteresis_preserves_small_limits_and_restores_configuration() {
        let mut controller = CacheController::default();
        let mut config =
            CacheConfig { inference_capacity: 3, memory_limit_mb: 100, ..Default::default() };
        let mut now = Instant::now();
        let sample = |mib: u64| Some(mib * 1024 * 1024);
        let limited = controller.update(&config, now, sample(110)).unwrap();
        assert_eq!(limited.inference, 1);
        assert_eq!(limited.parse, 32);
        assert!(
            controller.update(&config, now, sample(60)).is_none(),
            "do not resample every request"
        );
        now += Duration::from_secs(6);
        assert_eq!(controller.update(&config, now, sample(90)), Some(limited));
        now += Duration::from_secs(6);
        assert_eq!(controller.update(&config, now, sample(70)), Some(config.capacities()));
        now += Duration::from_secs(6);
        assert_eq!(controller.update(&config, now, sample(110)), Some(limited));
        config.memory_limit_mb = 0;
        assert_eq!(controller.update(&config, now, sample(110)), Some(config.capacities()));
        assert!(!controller.under_pressure);
    }

    #[test]
    fn unavailable_memory_measurement_does_not_invent_pressure() {
        let mut controller = CacheController::default();
        let config = CacheConfig { memory_limit_mb: 1, ..Default::default() };
        assert_eq!(controller.update(&config, Instant::now(), None), None);
        assert_eq!(controller.report()["underPressure"], false);
        assert!(controller.report()["lastSampledRssBytes"].is_null());
    }
}
