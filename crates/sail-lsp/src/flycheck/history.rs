//! Passive per-project check history; old successes never certify newer text.
use super::{cache::CacheInfo, Status};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunReport {
    pub toolchain: Option<super::toolchain::ToolchainReport>,
    pub compiler_success: bool,
    pub diagnostics_reported: usize,
    pub diagnostics_ignored: usize,
    pub elapsed_ms: u64,
    pub completed_at_unix_ms: u64,
    pub cache: CacheInfo,
}

impl RunReport {
    pub fn new(elapsed: Duration, cache: CacheInfo, compiler_success: bool) -> Self {
        Self {
            toolchain: None,
            compiler_success,
            diagnostics_reported: 0,
            diagnostics_ignored: 0,
            elapsed_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            completed_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            cache,
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckHistory {
    #[serde(skip)]
    started: Option<Instant>,
    pub cache: CacheInfo,
    pub last_run: Option<RunReport>,
    pub last_success: Option<RunReport>,
    pub last_success_current: bool,
}

impl CheckHistory {
    pub fn start(&mut self, cache: CacheInfo) {
        self.started = Some(Instant::now());
        self.cache = cache;
        self.last_success_current = false;
    }

    pub fn invalidate(&mut self) {
        self.started = None;
        self.last_success_current = false;
    }

    pub fn finish(&mut self, report: RunReport, status: &Status) {
        self.started = None;
        self.cache = report.cache.clone();
        self.last_success_current = matches!(status, Status::Passed | Status::Diagnostics { .. })
            && report.compiler_success;
        if self.last_success_current {
            self.last_success = Some(report.clone());
        }
        self.last_run = Some(report);
    }

    /// How long the current run has been going, while one is.
    ///
    /// The status line's first sentence, and the progress notification's
    /// whole message: a compiler run has no phases to report, so elapsed
    /// time is what the user watches.
    pub fn running_for(&self) -> Option<String> {
        self.started.map(|started| format!("Running for {}s.", started.elapsed().as_secs()))
    }

    pub fn summary(&self) -> String {
        let mut text = if let Some(running) = self.running_for() {
            format!("{running} ")
        } else if let Some(run) = &self.last_run {
            format!("Last run: {:.1}s. ", run.elapsed_ms as f64 / 1000.0)
        } else {
            String::new()
        };
        if let Some(path) = &self.cache.file {
            text.push_str(&format!(
                "Compiler cache: {} ({} at check start). ",
                path.display(),
                if self.cache.existing_at_start {
                    "existing memo file; edited goals may miss"
                } else {
                    "cold; the first full check can take minutes"
                }
            ));
        }
        if let Some(warning) = &self.cache.warning {
            text.push_str(warning);
            text.push(' ');
        }
        if let Some(toolchain) = self.last_run.as_ref().and_then(|run| run.toolchain.as_ref()) {
            if let Some(version) = &toolchain.version {
                text.push_str(&format!("Compiler: {version}. "));
            }
            for warning in &toolchain.warnings {
                text.push_str(warning);
                text.push(' ');
            }
        }
        if let Some(run) = self.last_run.as_ref().filter(|run| run.diagnostics_ignored != 0) {
            text.push_str(&format!(
                "Compiler reported {} findings ({} ignored by presentation settings). ",
                run.diagnostics_reported, run.diagnostics_ignored
            ));
        }
        if let Some(success) = &self.last_success {
            text.push_str(&format!(
                "Last successful check: {:.1}s ({}).",
                success.elapsed_ms as f64 / 1000.0,
                if self.last_success_current {
                    "current saved inputs"
                } else {
                    "older saved inputs"
                }
            ));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_and_rejected_or_cancelled_checks_do_not_recertify_an_old_success() {
        let mut history = CheckHistory::default();
        history.start(CacheInfo::default());
        history.finish(
            RunReport::new(Duration::from_secs(2), CacheInfo::default(), true),
            &Status::Passed,
        );
        assert!(history.last_success_current);
        history.invalidate();
        assert!(!history.last_success_current);
        assert!(history.last_success.is_some());
        history.start(CacheInfo::default());
        history.finish(
            RunReport::new(Duration::from_secs(1), CacheInfo::default(), false),
            &Status::TimedOut,
        );
        assert!(!history.last_success_current);
        assert_eq!(history.last_success.as_ref().unwrap().elapsed_ms, 2000);
        assert!(history.summary().contains("older saved inputs"));
    }
}
