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
    UnlocatedDiagnostics { count: usize, message: String },
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
        matches!(
            self,
            Self::Failed { .. }
                | Self::Skipped { .. }
                | Self::TimedOut
                | Self::UnlocatedDiagnostics { .. }
        )
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => f.write_str("Queued"),
            Self::Running => f.write_str("Running"),
            Self::Passed => f.write_str("Passed"),
            Self::Diagnostics { count } => write!(f, "Diagnostics: {count} reported"),
            Self::UnlocatedDiagnostics { count, message } => {
                write!(f, "Diagnostics: {count} reported; {message}")
            }
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
    config: crate::config::FlycheckConfig,
    generation: u64,
    args: Vec<String>,
    cwd: PathBuf,
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
        native_stdlib: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        let pending: Arc<Mutex<Option<Job>>> = Arc::default();
        let (wake, events) = crossbeam_channel::bounded(1);
        let queue = pending.clone();
        let join = thread::Builder::new().name("sail-flycheck".into()).spawn(move || {
            let mut capabilities = CapabilityCache::default();
            let mut toolchain = super::toolchain::ProbeCache::default();
            while events.recv().is_ok() {
                let job = queue.lock().unwrap().take();
                let Some(job) = job else { continue };
                let until = Instant::now() + Duration::from_millis(job.config.debounce_ms);
                while Instant::now() < until && !job.cancel.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                if job.cancel.load(Ordering::Acquire) {
                    continue;
                }
                let started = Instant::now();
                let timeout = Duration::from_millis(job.config.timeout_ms.max(1));
                let mut cache =
                    super::cache::PreparedCache::prepare(&job.config, &job.args, &job.cwd);
                let _ = sender.send(Task::FlycheckStarted {
                    target: target.clone(),
                    generation: job.generation,
                    cache: cache.info.clone(),
                });
                let pairing = toolchain.report(
                    &job.config.command,
                    &job.cwd,
                    native_stdlib.as_deref(),
                    &job.args,
                    |option| {
                        let remaining =
                            timeout.saturating_sub(started.elapsed()).min(Duration::from_secs(2));
                        let output = run_process(
                            &job.config.command,
                            &[option.into()],
                            &job.cwd,
                            remaining,
                            &job.cancel,
                        )
                        .map_err(|e| e.to_string())?;
                        if output.exit.success() && !output.truncated {
                            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
                        } else {
                            Err(output.text())
                        }
                    },
                );
                let (found, status) = run_with_format(
                    &job.config.command,
                    &cache.args,
                    &job.cwd,
                    timeout.saturating_sub(started.elapsed()),
                    &job.cancel,
                    job.config.diagnostic_format,
                    &mut capabilities,
                );
                let completed = matches!(
                    status,
                    Status::Passed
                        | Status::Diagnostics { .. }
                        | Status::UnlocatedDiagnostics { .. }
                );
                cache.finish(completed && !job.cancel.load(Ordering::Acquire));
                let success = matches!(status, Status::Passed | Status::Diagnostics { .. })
                    && !found.iter().any(|(_, diagnostic)| {
                        diagnostic.severity == Some(lsp_types::DiagnosticSeverity::ERROR)
                    });
                let mut report =
                    super::RunReport::new(started.elapsed(), cache.info.clone(), success);
                report.toolchain = Some(pairing);
                report.diagnostics_reported = match status {
                    Status::Diagnostics { count } | Status::UnlocatedDiagnostics { count, .. } => {
                        count
                    }
                    _ => found.len(),
                };
                let _ = sender.send(Task::Flycheck {
                    target: target.clone(),
                    generation: job.generation,
                    found,
                    status,
                    run: report,
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
            config: config.clone(),
            generation,
            args,
            cwd,
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

struct ProcessOutput {
    exit: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}
impl ProcessOutput {
    fn text(&self) -> String {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&self.stderr),
            String::from_utf8_lossy(&self.stdout)
        )
    }
}
#[derive(Default)]
struct CapabilityCache {
    key: Option<CompilerKey>,
    json_v1: bool,
}
#[derive(PartialEq, Eq, Hash)]
pub(super) struct CompilerKey {
    command: String,
    cwd: PathBuf,
    executable: Option<(PathBuf, u64, Option<std::time::SystemTime>)>,
}
impl CompilerKey {
    pub(super) fn executable_path(&self) -> Option<PathBuf> {
        self.executable.as_ref().map(|(path, _, _)| path.clone())
    }
    pub(super) fn new(command: &str, cwd: &std::path::Path) -> Self {
        let candidates: Vec<_> = if std::path::Path::new(command).components().count() > 1
            || std::path::Path::new(command).is_absolute()
        {
            vec![cwd.join(command)]
        } else {
            std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).map(|root| root.join(command)).collect())
                .unwrap_or_default()
        };
        let executable = candidates.into_iter().find_map(|path| {
            let path = if cfg!(windows) && !path.is_file() && path.extension().is_none() {
                path.with_extension("exe")
            } else {
                path
            };
            let metadata = std::fs::metadata(&path).ok()?;
            metadata.is_file().then(|| (path, metadata.len(), metadata.modified().ok()))
        });
        Self { command: command.into(), cwd: cwd.into(), executable }
    }
}

fn run_process(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<ProcessOutput, Status> {
    run_process_with_environment(command, args, cwd, timeout, cancel, &Default::default())
}

fn run_process_with_environment(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<ProcessOutput, Status> {
    if cancel.load(Ordering::Acquire) {
        return Err(Status::Cancelled);
    }
    let mut cmd = Command::new(command);
    cmd.args(args)
        .envs(environment)
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
    Ok(ProcessOutput { exit, stdout, stderr, truncated: out_cut || err_cut })
}

/// Editor fallback for clients without a task API. Same bounded pipe capture,
/// process-group termination and cancellation as compiler checks; no shell.
pub(crate) fn run_task(
    task: &crate::lsp_ext::ProcessRunnableArgs,
    timeout: Duration,
    cancel: &AtomicBool,
) -> String {
    match run_process_with_environment(
        &task.program,
        &task.args,
        &task.cwd,
        timeout,
        cancel,
        &task.environment,
    ) {
        Ok(output) => format!(
            "Exit: {}\n{}{}",
            output.exit,
            if output.truncated { "Output truncated to 1 MiB per stream\n" } else { "" },
            output.text()
        ),
        Err(status) => format!("Task: {status}\n"),
    }
}

#[cfg(test)]
pub(crate) fn run(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> (FlycheckResult, Status) {
    run_with_format(
        command,
        args,
        cwd,
        timeout,
        cancel,
        crate::config::CompilerDiagnosticFormat::Text,
        &mut CapabilityCache::default(),
    )
}

fn run_with_format(
    command: &str,
    args: &[String],
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &AtomicBool,
    format: crate::config::CompilerDiagnosticFormat,
    capabilities: &mut CapabilityCache,
) -> (FlycheckResult, Status) {
    use crate::config::CompilerDiagnosticFormat as Format;
    let started = Instant::now();
    let remaining =
        |started: Instant| timeout.checked_sub(started.elapsed()).ok_or(Status::TimedOut);
    if args.iter().any(|arg| {
        arg.starts_with("--diagnostics-format") || arg.starts_with("-diagnostics_format")
    }) {
        return (vec![],Status::Skipped{message:"Set flycheck.diagnosticFormat instead of supplying diagnostic protocol flags in flycheck.extraArgs".into()});
    }
    if args.iter().any(|arg| arg == "--check-context") {
        match run_process(command,&["--check-context-version".into()],cwd,timeout.min(Duration::from_secs(5)),cancel) {
            Ok(output) if output.exit.success() && !output.truncated && String::from_utf8_lossy(&output.stdout).trim()=="1" => {},
            Ok(_) => return (vec![],Status::Skipped{message:"This configuration requires Sail check-context protocol v1 (target/includePaths/generatedFiles/typed variables). Install a compiler with the supplied check-context patch, then retry.".into()}),
            Err(status) => return (vec![],status),
        }
    }
    let mut json = false;
    if format != Format::Text {
        let key = CompilerKey::new(command, cwd);
        if capabilities.key.as_ref() != Some(&key) {
            let budget = match remaining(started) {
                Ok(value) => value,
                Err(status) => return (vec![], status),
            };
            match run_process(
                command,
                &["--diagnostics-format-version".into()],
                cwd,
                budget.min(Duration::from_secs(5)),
                cancel,
            ) {
                Ok(output) if !output.truncated => {
                    capabilities.json_v1 = output.exit.success()
                        && String::from_utf8_lossy(&output.stdout).trim() == "1";
                    capabilities.key = Some(key);
                }
                Ok(_) => {
                    return (
                        vec![],
                        Status::Failed {
                            message: "Compiler diagnostic capability output exceeded 1 MiB".into(),
                        },
                    )
                }
                Err(status) => return (vec![], status),
            }
        }
        json = capabilities.json_v1;
        if format == Format::Json && !json {
            return(vec![],Status::Skipped{message:"flycheck.diagnosticFormat=json requires Sail editor diagnostic protocol v1; the selected compiler does not advertise it".into()});
        }
    }
    let mut args = args.to_vec();
    if json {
        args.extend(["--diagnostics-format".into(), "json-v1".into()]);
    }
    let budget = match remaining(started) {
        Ok(value) => value,
        Err(status) => return (vec![], status),
    };
    let output = match run_process(command, &args, cwd, budget, cancel) {
        Ok(output) => output,
        Err(status) => return (vec![], status),
    };
    if output.truncated {
        return (
            vec![],
            Status::Failed {
                message: "Compiler output exceeded 1 MiB; diagnostics may be incomplete".into(),
            },
        );
    }
    if json {
        let stdout = match std::str::from_utf8(&output.stdout) {
            Ok(text) => text,
            Err(error) => {
                return (
                    vec![],
                    Status::Failed {
                        message: format!("Compiler JSON output is not UTF-8: {error}"),
                    },
                )
            }
        };
        return match super::structured::parse(stdout, output.exit.success()) {
            Ok(report) => {
                let count = report.found.len() + report.unlocated.len();
                let status = if !report.success && report.errors == 0 {
                    Status::Failed {
                        message: format!(
                            "Compiler check failed without an error diagnostic: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        ),
                    }
                } else if !report.unlocated.is_empty() {
                    Status::UnlocatedDiagnostics { count, message: report.unlocated.join("\n") }
                } else if count == 0 {
                    Status::Passed
                } else {
                    Status::Diagnostics { count }
                };
                (report.found, status)
            }
            Err(super::structured::Error::Stale(message)) => {
                log::warn!("{message}");
                (vec![], Status::Stale)
            }
            Err(error) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail: String = stderr.trim().chars().take(2048).collect();
                (
                    vec![],
                    Status::Failed {
                        message: if detail.is_empty() {
                            error.to_string()
                        } else {
                            format!("{error}. Compiler stderr: {detail}")
                        },
                    },
                )
            }
        };
    }
    let text = output.text();
    let found = super::parse(&text, cwd);
    let status = if !output.exit.success() && found.is_empty() {
        Status::Failed {
            message: format!(
                "{command:?} exited with {} without readable diagnostics: {}",
                output.exit,
                text.trim().chars().take(2048).collect::<String>()
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
    #[test]
    #[ignore = "requires SAIL_ORACLE_BIN and SAIL_ORACLE_SRC; reads and writes real Sail memo files"]
    fn managed_cache_round_trips_the_real_compiler_memo_format() {
        let _environment =
            crate::server_check::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let compiler = std::env::var("SAIL_ORACLE_BIN").expect("SAIL_ORACLE_BIN required");
        let library = std::env::var("SAIL_ORACLE_SRC").expect("SAIL_ORACLE_SRC required");
        std::fs::write(
            root.path().join("main.sail"),
            "default Order dec\n$include <prelude.sail>\nfunction identity(x : int) -> int = x\n",
        )
        .unwrap();
        let config = crate::config::FlycheckConfig {
            command: compiler.clone(),
            cache_directory: Some("memo-cache".into()),
            ..Default::default()
        };
        let args = vec!["--just-check".into(), "main.sail".into()];
        for warm in [false, true] {
            let mut cache =
                super::super::cache::PreparedCache::prepare(&config, &args, root.path());
            assert_eq!(cache.info.existing_at_start, warm);
            let result = run_process_with_environment(
                &compiler,
                &cache.args,
                root.path(),
                Duration::from_secs(30),
                &AtomicBool::new(false),
                &[("SAIL_DIR".into(), library.clone())].into_iter().collect(),
            )
            .unwrap_or_else(|error| panic!("{error}"));
            assert!(result.exit.success(), "{}", result.text());
            cache.finish(true);
            assert!(cache.info.bytes_after_check > 0);
            assert!(cache.info.warning.is_none(), "{:?}", cache.info);
        }
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
        let mut worker = Worker::new(CheckTarget::File(dir.join("a.sail")), sender, None).unwrap();
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

#[cfg(all(test, unix))]
mod structured_worker_tests {
    use super::*;
    use crate::config::CompilerDiagnosticFormat as Format;
    use std::os::unix::fs::PermissionsExt;
    fn install(directory: &std::path::Path, supported: bool) -> PathBuf {
        let path = directory.join("sail");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = --diagnostics-format-version ]; then
  printf p >> probes
  {}
fi
printf r >> runs
{}
"#,
            if supported { "echo 1; exit 0" } else { "exit 2" },
            if supported {
                r#"printf '%s\n' '{"protocol":"sail-editor-diagnostics","version":1,"event":"summary","success":true,"diagnostics":0}'"#
            } else {
                "exit 0"
            }
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn check(
        path: &std::path::Path,
        format: Format,
        cache: &mut CapabilityCache,
    ) -> (FlycheckResult, Status) {
        let start = Instant::now();
        loop {
            let result = run_with_format(
                path.to_str().unwrap(),
                &["--just-check".into()],
                path.parent().unwrap(),
                Duration::from_secs(3),
                &AtomicBool::new(false),
                format,
                cache,
            );
            if matches!(&result.1,Status::Failed{message} if message.contains("os error 26"))
                && start.elapsed() < Duration::from_secs(3)
            {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            return result;
        }
    }
    #[test]
    fn negotiation_is_cached_and_binary_changes_require_a_new_probe() {
        let directory =
            std::env::temp_dir().join(format!("sail-format-cache-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = install(&directory, true);
        let mut cache = CapabilityCache::default();
        assert_eq!(check(&path, Format::Auto, &mut cache).1, Status::Passed);
        assert_eq!(check(&path, Format::Json, &mut cache).1, Status::Passed);
        assert_eq!(std::fs::read_to_string(directory.join("probes")).unwrap(), "p");
        let path = install(&directory, false);
        assert!(matches!(check(&path, Format::Json, &mut cache).1, Status::Skipped { .. }));
        assert_eq!(std::fs::read_to_string(directory.join("runs")).unwrap(), "rr");
        assert_eq!(check(&path, Format::Auto, &mut cache).1, Status::Passed);
        assert_eq!(std::fs::read_to_string(directory.join("probes")).unwrap(), "pp");
        assert_eq!(std::fs::read_to_string(directory.join("runs")).unwrap(), "rrr");
        assert_eq!(check(&path, Format::Text, &mut CapabilityCache::default()).1, Status::Passed);
        assert_eq!(std::fs::read_to_string(directory.join("probes")).unwrap(), "pp");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
