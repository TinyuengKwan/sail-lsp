//! A serial, replaceable check queue per project. Compiler processes never
//! occupy the IDE request pool; replacing a save cancels and reaps its process.
use super::{CheckTarget, FlycheckResult};
use crate::main_loop::Task;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Queued,
    Running,
    Passed,
    Diagnostics { count: usize },
    Failed { message: String },
    Skipped { message: String },
    Cancelled,
    Stale,
    TimedOut,
}
impl Status {
    pub(crate) fn busy(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
    pub(crate) fn problem(&self) -> bool {
        matches!(self, Self::Failed { .. } | Self::Skipped { .. } | Self::TimedOut)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => f.write_str("Queued"),
            Self::Running => f.write_str("Running"),
            Self::Passed => f.write_str("Passed"),
            Self::Diagnostics { count } => write!(f, "Diagnostics: {count} reported"),
            Self::Failed { message } => write!(f, "Failed: {message}"),
            Self::Skipped { message } => write!(f, "Skipped: {message}"),
            Self::Cancelled => f.write_str("Cancelled"),
            Self::Stale => {
                f.write_str("Source changed; save affected files to refresh compiler diagnostics")
            }
            Self::TimedOut => f.write_str("Timed out; increase flycheck.timeoutMs or retry"),
        }
    }
}

struct Job {
    generation: u64,
    command: String,
    args: Vec<String>,
    cwd: PathBuf,
    timeout: Duration,
    debounce: Duration,
    cancel: Arc<AtomicBool>,
}
pub(crate) struct Worker {
    pending: Arc<Mutex<Option<Job>>>,
    wake: Option<crossbeam_channel::Sender<()>>,
    cancel: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub(crate) fn new(
        target: CheckTarget,
        sender: crossbeam_channel::Sender<Task>,
    ) -> std::io::Result<Self> {
        let pending: Arc<Mutex<Option<Job>>> = Arc::default();
        let (wake, events) = crossbeam_channel::bounded(1);
        let queue = pending.clone();
        let join = thread::Builder::new().name("sail-flycheck".into()).spawn(move || {
            while events.recv().is_ok() {
                let job = queue.lock().unwrap().take();
                let Some(job) = job else { continue };
                let until = Instant::now() + job.debounce;
                while Instant::now() < until && !job.cancel.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                if job.cancel.load(Ordering::Acquire) {
                    continue;
                }
                let _ = sender.send(Task::FlycheckStarted {
                    target: target.clone(),
                    generation: job.generation,
                });
                let (found, status) =
                    run(&job.command, &job.args, &job.cwd, job.timeout, &job.cancel);
                let _ = sender.send(Task::Flycheck {
                    target: target.clone(),
                    generation: job.generation,
                    found,
                    status,
                });
            }
        })?;
        Ok(Self { pending, wake: Some(wake), cancel: Arc::default(), join: Some(join) })
    }
    pub(crate) fn submit(
        &mut self,
        generation: u64,
        config: &crate::config::FlycheckConfig,
        args: Vec<String>,
        cwd: PathBuf,
    ) {
        self.cancel();
        self.cancel = Arc::default();
        *self.pending.lock().unwrap() = Some(Job {
            generation,
            command: config.command.clone(),
            args,
            cwd,
            timeout: Duration::from_millis(config.timeout_ms.max(1)),
            debounce: Duration::from_millis(config.debounce_ms),
            cancel: self.cancel.clone(),
        });
        let _ = self.wake.as_ref().unwrap().try_send(());
    }
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.pending.lock().unwrap().take();
        self.wake.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Drain both pipes concurrently to avoid stdout/stderr deadlocks. Bound retained
/// output while still draining the pipe so a noisy compiler cannot block exit.
fn reader(
    mut pipe: impl Read + Send + 'static,
) -> thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>> {
    thread::spawn(move || {
        let mut saved = Vec::new();
        let mut buffer = [0; 8192];
        let mut truncated = false;
        loop {
            let n = pipe.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            let keep = n.min((1024 * 1024usize).saturating_sub(saved.len()));
            saved.extend_from_slice(&buffer[..keep]);
            truncated |= keep < n;
        }
        Ok((saved, truncated))
    })
}
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: the child started in its own process group; the negative PID
    // addresses only that group. Kill descendants holding inherited pipes too.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_process(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(std::process::ExitStatus, String, bool), Status> {
    if cancel.load(Ordering::Acquire) {
        return Err(Status::Cancelled);
    }
    let mut cmd = Command::new(command);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(Status::Failed {
                message: format!("Could not start {command:?} in {}: {error}", cwd.display()),
            })
        }
    };
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let start = Instant::now();
    let result = loop {
        if cancel.load(Ordering::Acquire) {
            break Err(Status::Cancelled);
        }
        if start.elapsed() >= timeout {
            break Err(Status::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                break Err(Status::Failed { message: format!("Waiting for {command:?}: {error}") })
            }
        }
    };
    terminate(&mut child);
    let stdout = stdout.join().ok().and_then(Result::ok);
    let stderr = stderr.join().ok().and_then(Result::ok);
    let exit = result?;
    let (Some((stdout, out_cut)), Some((stderr, err_cut))) = (stdout, stderr) else {
        return Err(Status::Failed { message: "Could not read compiler output".into() });
    };
    let text =
        format!("{}\n{}", String::from_utf8_lossy(&stderr), String::from_utf8_lossy(&stdout));
    Ok((exit, text, out_cut || err_cut))
}

pub(crate) fn run(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> (FlycheckResult, Status) {
    let started = Instant::now();
    if args.iter().any(|arg| arg == "--check-context") {
        match run_process(command, &["--check-context-version".into()], cwd, timeout.min(Duration::from_secs(5)), cancel) {
            Ok((exit, output, false)) if exit.success() && output.trim() == "1" => {}
            Ok(_) => return (vec![], Status::Skipped { message: "This configuration requires Sail check-context protocol v1 (target/includePaths/generatedFiles/typed variables). Install a compiler with the supplied check-context patch, then retry.".into() }),
            Err(status) => return (vec![], status),
        }
    }
    let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
        return (vec![], Status::TimedOut);
    };
    let (exit, text, truncated) = match run_process(command, args, cwd, remaining, cancel) {
        Ok(result) => result,
        Err(status) => return (vec![], status),
    };
    let found = super::parse(&text, cwd);
    let status = if truncated {
        Status::Failed {
            message: "Compiler output exceeded 1 MiB; diagnostics may be incomplete".into(),
        }
    } else if !exit.success() && found.is_empty() {
        let detail: String = text.trim().chars().take(2048).collect();
        Status::Failed {
            message: format!(
                "{command:?} exited with {exit} without readable diagnostics: {detail}"
            ),
        }
    } else if found.is_empty() {
        Status::Passed
    } else {
        Status::Diagnostics { count: found.len() }
    };
    (found, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn check_context_negotiation_never_runs_an_unsupported_configuration() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("sail-check-capability-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let command = dir.join("compiler");
        let ran = dir.join("checked");
        let _ = std::fs::remove_file(&ran);
        let install = |probe: &str| {
            std::fs::write(&command, format!("#!/bin/sh\nif [ \"$1\" = --check-context-version ]; then\n{probe}\nfi\nprintf checked > checked\nexit 0\n")).unwrap();
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let check = |timeout: Duration| {
            let start = Instant::now();
            loop {
                let remaining = timeout.saturating_sub(start.elapsed());
                let result = run(
                    command.to_str().unwrap(),
                    &["--check-context".into(), "{}".into()],
                    &dir,
                    remaining,
                    &AtomicBool::new(false),
                );
                // Another concurrently spawning test can briefly inherit a
                // write descriptor for this freshly written script. Retry
                // only Linux ETXTBSY, within the original test deadline.
                if matches!(&result.1, Status::Failed { message } if message.contains("os error 26"))
                    && start.elapsed() < timeout
                {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                return result;
            }
        };
        for probe in ["echo 'unknown option' >&2; exit 2", "echo 2; exit 0"] {
            install(probe);
            let (_, status) = check(Duration::from_secs(2));
            assert!(matches!(status, Status::Skipped { .. }), "{probe}: {status:?}");
            assert!(!ran.exists(), "unsupported context must never be checked as defaults");
        }
        install("sleep 30; exit 0");
        assert_eq!(check(Duration::from_millis(100)).1, Status::TimedOut);
        assert!(!ran.exists());
        install("echo 1; exit 0");
        assert_eq!(check(Duration::from_secs(2)).1, Status::Passed);
        assert!(ran.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_compiler_is_a_failure_not_a_clean_check() {
        let (found, status) = run(
            "/no/such/sail-compiler",
            &[],
            &std::env::temp_dir(),
            Duration::from_secs(1),
            &AtomicBool::new(false),
        );
        assert!(found.is_empty());
        assert!(
            matches!(status, Status::Failed { message } if message.contains("Could not start"))
        );
    }
    #[cfg(unix)]
    #[test]
    fn exit_status_unlocated_errors_timeout_and_large_output_are_distinct() {
        let check = |script: &str, timeout| {
            run(
                "/bin/sh",
                &["-c".into(), script.into()],
                &std::env::temp_dir(),
                timeout,
                &AtomicBool::new(false),
            )
        };
        assert_eq!(check("exit 0", Duration::from_secs(2)).1, Status::Passed);
        assert!(
            matches!(check("echo 'invalid command line' >&2; exit 2", Duration::from_secs(2)).1, Status::Failed { message } if message.contains("invalid command line"))
        );
        let start = Instant::now();
        assert_eq!(check("sleep 30", Duration::from_millis(100)).1, Status::TimedOut);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "process and its inherited pipes must be reaped"
        );
        assert!(
            matches!(check("head -c 1200000 /dev/zero", Duration::from_secs(3)).1, Status::Failed { message } if message.contains("exceeded"))
        );
        let (found, state) = check(
            "printf 'Type error:\na.sail:1.1-2:\n  | mismatch\n' >&2; exit 1",
            Duration::from_secs(2),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(state, Status::Diagnostics { count: 1 });
    }

    #[cfg(windows)]
    #[test]
    fn windows_compiler_timeout_reaps_descendants_and_inherited_pipes() {
        let started = Instant::now();
        // cmd and its ping child both inherit the captured pipes. Killing
        // just cmd leaves the readers waiting for the descendant to exit.
        let (_, status) = run(
            "cmd.exe",
            &["/D".into(), "/C".into(), "ping -n 30 127.0.0.1 >nul".into()],
            &std::env::temp_dir(),
            Duration::from_millis(500),
            &AtomicBool::new(false),
        );
        assert_eq!(status, Status::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10), "descendant retained the pipes");
    }

    #[cfg(windows)]
    #[test]
    fn windows_compiler_cancellation_and_exit_status_are_distinct() {
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                cancel.store(true, Ordering::Release);
            });
            let (_, status) = run(
                "cmd.exe",
                &["/D".into(), "/C".into(), "ping -n 30 127.0.0.1 >nul".into()],
                &std::env::temp_dir(),
                Duration::from_secs(10),
                &cancel,
            );
            assert_eq!(status, Status::Cancelled);
        });
        let (_, status) = run(
            "cmd.exe",
            &["/D".into(), "/C".into(), "exit 0".into()],
            &std::env::temp_dir(),
            Duration::from_secs(10),
            &AtomicBool::new(false),
        );
        assert_eq!(status, Status::Passed);
    }
    #[cfg(unix)]
    #[test]
    fn repeated_saves_coalesce_and_a_running_check_is_reaped_before_replacement() {
        let dir = std::env::temp_dir().join(format!("sail-check-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("runs");
        let _ = std::fs::remove_file(&log);
        let (sender, events) = crossbeam_channel::unbounded();
        let mut worker = Worker::new(CheckTarget::File(dir.join("a.sail")), sender).unwrap();
        let mut config = crate::config::FlycheckConfig {
            command: "/bin/sh".into(),
            debounce_ms: 100,
            ..Default::default()
        };
        let args = |script: &str, generation: u64| {
            vec![
                "-c".into(),
                script.into(),
                "test".into(),
                generation.to_string(),
                log.to_string_lossy().into_owned(),
            ]
        };
        for generation in 1..=10 {
            worker.submit(
                generation,
                &config,
                args("echo \"$1\" >> \"$2\"", generation),
                dir.clone(),
            );
        }
        loop {
            if let Task::Flycheck { generation, status, .. } =
                events.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                assert_eq!(generation, 10);
                assert_eq!(status, Status::Passed);
                break;
            }
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "10\n");
        config.debounce_ms = 0;
        worker.submit(
            11,
            &config,
            args("echo \"$1\" >> \"$2\"; sleep 30; echo stale >> \"$2\"", 11),
            dir.clone(),
        );
        let start = Instant::now();
        while !std::fs::read_to_string(&log).unwrap().contains("11\n") {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
        worker.submit(12, &config, args("echo \"$1\" >> \"$2\"", 12), dir.clone());
        let mut cancelled = false;
        loop {
            if let Task::Flycheck { generation, status, .. } =
                events.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                if generation == 11 {
                    assert_eq!(status, Status::Cancelled);
                    cancelled = true;
                }
                if generation == 12 {
                    assert_eq!(status, Status::Passed);
                    break;
                }
            }
        }
        assert!(cancelled);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "10\n11\n12\n");
        worker.submit(13, &config, args("sleep 30", 13), dir.clone());
        let start = Instant::now();
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(3));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
