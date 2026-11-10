//! An implementation of `loader::Handle`, based on `walkdir` and `notify`.
//!
//! Native subscriptions cover explicit dependencies, including missing files
//! outside workspace folders. Parent watches and reads after registration
//! preserve updates across file replacement and directory recreation.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};

use crossbeam_channel::{select, unbounded, Receiver, Sender};
use notify::{event::AccessKind, Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use paths::{AbsPath, AbsPathBuf, Utf8PathBuf};
use rayon::iter::{IntoParallelIterator as _, ParallelIterator};
use rustc_hash::FxHashSet;
use vfs::loader::{self, LoadingProgress};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct NotifyHandle {
    // Relative order of fields below is significant.
    sender: Sender<Message>,
    _thread: stdx::thread::JoinHandle,
}

#[derive(Debug)]
enum Message {
    Config(loader::Config),
    Invalidate(AbsPathBuf),
}

impl loader::Handle for NotifyHandle {
    fn spawn(sender: loader::Sender) -> NotifyHandle {
        let actor = NotifyActor::new(sender);
        let (sender, receiver) = unbounded::<Message>();
        let thread = stdx::thread::Builder::new(stdx::thread::ThreadIntent::Worker, "VfsLoader")
            .spawn(move || actor.run(receiver))
            .expect("failed to spawn thread");
        NotifyHandle { sender, _thread: thread }
    }

    fn set_config(&mut self, config: loader::Config) {
        self.sender.send(Message::Config(config)).unwrap();
    }

    fn invalidate(&mut self, path: AbsPathBuf) {
        self.sender.send(Message::Invalidate(path)).unwrap();
    }

    fn load_sync(&mut self, path: &AbsPath) -> Option<Vec<u8>> {
        read(path)
    }
}

type NotifyEvent = notify::Result<notify::Event>;
type WatcherFactory = fn(Sender<NotifyEvent>) -> notify::Result<RecommendedWatcher>;

struct NotifyActor {
    sender: loader::Sender,
    watched_file_entries: FxHashSet<AbsPathBuf>,
    watched_dir_entries: Vec<loader::Directories>,
    seen_paths: FxHashSet<AbsPathBuf>,
    // Native backends may report canonical roots (e.g. /private/var for /var).
    // Restore the subscribed spelling before matching or publishing VFS paths.
    watch_aliases: Vec<(AbsPathBuf, AbsPathBuf)>,
    watch_roots: FxHashSet<AbsPathBuf>,
    config_version: u32,
    watch_error: Option<String>,
    last_watch_status: Option<(u32, Option<String>)>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    watcher_factory: WatcherFactory,
    // Drop order is significant.
    watcher: Option<(RecommendedWatcher, Receiver<NotifyEvent>)>,
}

#[derive(Debug)]
enum Event {
    Message(Message),
    NotifyEvent(NotifyEvent),
    RetryWatch,
}

impl NotifyActor {
    fn new(sender: loader::Sender) -> NotifyActor {
        NotifyActor {
            sender,
            watched_dir_entries: Vec::new(),
            watched_file_entries: FxHashSet::default(),
            seen_paths: FxHashSet::default(),
            watch_aliases: Vec::new(),
            watch_roots: FxHashSet::default(),
            config_version: 0,
            watch_error: None,
            last_watch_status: None,
            retry_at: None,
            retry_delay: Duration::from_secs(1),
            watcher_factory: native_watcher,
            watcher: None,
        }
    }

    fn next_event(&self, receiver: &Receiver<Message>) -> Option<Event> {
        let watcher = self
            .watcher
            .as_ref()
            .map(|(_, rx)| rx.clone())
            .unwrap_or_else(crossbeam_channel::never);
        let retry = self
            .retry_at
            .map(|at| crossbeam_channel::after(at.saturating_duration_since(Instant::now())))
            .unwrap_or_else(crossbeam_channel::never);
        select! {
            recv(receiver) -> it => it.ok().map(Event::Message),
            recv(watcher) -> it => Some(Event::NotifyEvent(it.unwrap_or_else(|_| Err(notify::Error::generic("native watcher channel closed"))))),
            recv(retry) -> _ => Some(Event::RetryWatch),
        }
    }

    fn run(mut self, inbox: Receiver<Message>) {
        while let Some(event) = self.next_event(&inbox) {
            tracing::debug!(?event, "vfs-notify event");
            match event {
                Event::Message(msg) => match msg {
                    Message::Config(config) => {
                        self.config_version = config.version;
                        self.watch_error = None;
                        self.watch_roots.clear();
                        self.watcher = None;
                        if !config.watch.is_empty() {
                            self.create_watcher();
                        } else {
                            self.retry_at = None;
                            self.retry_delay = Duration::from_secs(1);
                        }

                        let config_version = config.version;

                        let n_total = config.load.len();
                        self.watched_dir_entries.clear();
                        self.watched_file_entries.clear();
                        self.watch_aliases.clear();

                        self.send(loader::Message::Progress {
                            n_total,
                            n_done: LoadingProgress::Started,
                            config_version,
                            dir: None,
                        });

                        // Subscribe before reading: a change between the disk read
                        // and registration must not disappear. Parent watches survive
                        // atomic replacement and observe previously missing paths.
                        let mut watch_roots = FxHashSet::default();
                        for &index in &config.watch {
                            match &config.load[index] {
                                loader::Entry::Files(files) => {
                                    self.seen_paths.extend(files.iter().cloned());
                                    self.watched_file_entries.extend(files.iter().cloned());
                                    for file in files {
                                        if let Some(parent) = existing_watch_parent(file.as_ref()) {
                                            watch_roots.insert(parent.to_path_buf());
                                        }
                                    }
                                }
                                loader::Entry::Directories(dirs)
                                | loader::Entry::WatchDirectories(dirs) => {
                                    self.watched_dir_entries.push(dirs.clone());
                                    for root in &dirs.include {
                                        let path: &Path = root.as_ref();
                                        if let Some(parent) = path.ancestors().find(|p| p.is_dir())
                                        {
                                            watch_roots.insert(parent.to_path_buf());
                                        }
                                    }
                                }
                            }
                        }
                        // Loading a project reads its open, unlinked sources too.
                        // Forgetting these paths on reconfiguration makes each
                        // read look like a newly discovered FUSE file, causing
                        // another project reload and another reconfiguration.
                        // Retain only paths covered by the new subscriptions.
                        self.seen_paths.retain(|path| {
                            self.watched_file_entries.contains(path)
                                || self
                                    .watched_dir_entries
                                    .iter()
                                    .any(|dir| dir.contains_file(path))
                        });
                        for path in watch_roots {
                            self.watch(&path);
                        }
                        self.finish_watch_attempt();
                        let processed = AtomicUsize::new(0);
                        config.load.into_par_iter().for_each(|entry| {
                            let files = Self::load_entry(entry, |file| {
                                self.send(loader::Message::Progress {
                                    n_total,
                                    n_done: LoadingProgress::Progress(
                                        processed.load(std::sync::atomic::Ordering::Relaxed),
                                    ),
                                    dir: Some(file),
                                    config_version,
                                });
                            });
                            self.send(loader::Message::Loaded { files });
                            self.send(loader::Message::Progress {
                                n_total,
                                n_done: LoadingProgress::Progress(
                                    processed.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1,
                                ),
                                config_version,
                                dir: None,
                            });
                        });

                        self.send(loader::Message::Progress {
                            n_total,
                            n_done: LoadingProgress::Finished,
                            config_version,
                            dir: None,
                        });
                    }
                    Message::Invalidate(path) => {
                        let contents = read(path.as_path());
                        let files = vec![(path, contents)];
                        self.send(loader::Message::Changed { files });
                    }
                },
                Event::RetryWatch => self.retry_watches(),
                Event::NotifyEvent(event) => {
                    let event = match event {
                        Ok(event) => event,
                        Err(error) => {
                            self.watcher = None;
                            self.watch_failed(error.to_string());
                            self.request_rescan(Vec::new());
                            continue;
                        }
                    };
                    if event.need_rescan() {
                        self.request_rescan(self.event_paths(event.paths));
                        continue;
                    }
                    if matches!(
                        event.kind,
                        EventKind::Create(_)
                            | EventKind::Modify(_)
                            | EventKind::Remove(_)
                            | EventKind::Access(AccessKind::Open(_))
                    ) {
                        let mut abs_paths = self.event_paths(event.paths);

                        if matches!(event.kind, EventKind::Access(_)) {
                            abs_paths.retain(|path| !(path.as_ref() as &Path).is_dir());
                        }

                        // A directory may be created, removed or renamed as a unit.
                        // Re-read explicitly watched descendants, including paths that
                        // were missing when the watch was registered.
                        let descendants: Vec<_> = self
                            .watched_file_entries
                            .iter()
                            .filter(|file| {
                                abs_paths.iter().any(|p| *file != p && file.starts_with(p))
                            })
                            .cloned()
                            .collect();
                        // Recursive backends may deliver a directory event
                        // before installing watches on its children. A file
                        // created after our read but before that installation
                        // would then produce no event. Synchronize registration
                        // with the backend before re-reading explicit children.
                        // Also handle a removed directory already recreated by
                        // the time its queued removal reaches this actor.
                        if matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        ) {
                            let parents: FxHashSet<_> = descendants
                                .iter()
                                .filter_map(|file| {
                                    existing_watch_parent(file.as_ref()).map(Path::to_path_buf)
                                })
                                .collect();
                            for parent in parents {
                                self.watch(&parent);
                            }
                        }
                        abs_paths.extend(descendants);
                        abs_paths.sort();
                        abs_paths.dedup();

                        // Only consider access events for files that we haven't seen
                        // before.
                        //
                        // This is important on FUSE filesystems, where we may not get a
                        // Create event. In other cases we're about to access the file, so
                        // we don't want an infinite loop where processing an Access event
                        // creates another Access event.
                        abs_paths.retain(|path| {
                            let watched = self.watched_file_entries.contains(path)
                                || self
                                    .watched_dir_entries
                                    .iter()
                                    .any(|dir| dir.contains_file(path));
                            watched
                                && (self.seen_paths.insert(path.clone())
                                    || !matches!(event.kind, EventKind::Access(_)))
                        });

                        let files: Vec<_> = abs_paths
                            .into_iter()
                            .filter_map(|path| -> Option<(AbsPathBuf, Option<Vec<u8>>)> {
                                if self.watched_file_entries.contains(&path) {
                                    return Some((path.clone(), read(&path)));
                                }

                                // For removed files, fs::metadata() will return Err, but
                                // we still want to update the VFS.
                                if matches!(event.kind, EventKind::Remove(_)) {
                                    return Some((path, None));
                                }

                                let meta = fs::metadata(&path).ok()?;
                                if meta.file_type().is_dir()
                                    && self
                                        .watched_dir_entries
                                        .iter()
                                        .any(|dir| dir.contains_dir(&path))
                                {
                                    self.watch(path.as_ref());
                                    return None;
                                }

                                if !meta.file_type().is_file() {
                                    return None;
                                }

                                let contents = read(&path);
                                Some((path, contents))
                            })
                            .collect();
                        if !files.is_empty() {
                            self.send(loader::Message::Changed { files });
                        }
                    }
                }
            }
        }
    }

    fn load_entry(
        entry: loader::Entry,
        send_message: impl Fn(AbsPathBuf),
    ) -> Vec<(AbsPathBuf, Option<Vec<u8>>)> {
        match entry {
            loader::Entry::Files(files) => files
                .into_iter()
                .map(|file| {
                    let contents = read(file.as_path());
                    (file, contents)
                })
                .collect::<Vec<_>>(),
            loader::Entry::WatchDirectories(_) => Vec::new(),
            loader::Entry::Directories(dirs) => {
                let mut res = Vec::new();

                for root in &dirs.include {
                    send_message(root.clone());
                    let walkdir =
                        WalkDir::new(root).follow_links(true).into_iter().filter_entry(|entry| {
                            if !entry.file_type().is_dir() {
                                return true;
                            }
                            let path = entry.path();

                            if path_might_be_cyclic(path) {
                                return false;
                            }

                            // We want to filter out subdirectories that are roots themselves, because they will be visited separately.
                            dirs.exclude.iter().all(|it| it != path)
                                && (root == path || dirs.include.iter().all(|it| it != path))
                        });

                    let files = walkdir.filter_map(|it| it.ok()).filter_map(|entry| {
                        let depth = entry.depth();
                        let is_dir = entry.file_type().is_dir();
                        let is_file = entry.file_type().is_file();
                        let abs_path = AbsPathBuf::try_from(
                            Utf8PathBuf::from_path_buf(entry.into_path()).ok()?,
                        )
                        .ok()?;
                        if depth < 2 && is_dir {
                            send_message(abs_path.clone());
                        }
                        if !is_file {
                            return None;
                        }
                        let ext = abs_path.extension().unwrap_or_default();
                        if dirs.extensions.iter().all(|it| it.as_str() != ext) {
                            return None;
                        }
                        Some(abs_path)
                    });

                    res.extend(files.map(|file| {
                        let contents = read(file.as_path());
                        (file, contents)
                    }));
                }
                res
            }
        }
    }

    fn event_paths(&self, paths: impl IntoIterator<Item = PathBuf>) -> Vec<AbsPathBuf> {
        let mut result = Vec::new();
        for path in paths
            .into_iter()
            .filter_map(|p| AbsPathBuf::try_from(Utf8PathBuf::from_path_buf(p).ok()?).ok())
        {
            let path = path.normalize();
            if self.watch_aliases.is_empty() {
                result.push(path);
                continue;
            }
            // Some backends return canonical paths, others the last alias
            // registered for an inode. Expand through the registered native
            // witness so every subscribed spelling receives the same update.
            let mut native = vec![path.clone()];
            native.extend(self.watch_aliases.iter().filter_map(|(physical, original)| {
                Some(physical.join(path.strip_prefix(original)?))
            }));
            native.sort();
            native.dedup();
            for path in native {
                result.extend(self.watch_aliases.iter().filter_map(|(physical, original)| {
                    Some(original.join(path.strip_prefix(physical)?))
                }));
                result.push(path);
            }
        }
        result.sort();
        result.dedup();
        result
    }

    fn create_watcher(&mut self) {
        let (sender, receiver) = unbounded();
        match (self.watcher_factory)(sender) {
            Ok(watcher) => self.watcher = Some((watcher, receiver)),
            Err(error) => self.watch_failed(error.to_string()),
        }
    }

    fn publish_watch_status(&mut self) {
        let status = (self.config_version, self.watch_error.clone());
        if self.last_watch_status.as_ref() != Some(&status) {
            self.last_watch_status = Some(status.clone());
            self.send(loader::Message::WatchStatus { config_version: status.0, error: status.1 });
        }
    }

    fn watch_failed(&mut self, error: String) {
        tracing::warn!(%error, "native watcher degraded; scheduling recovery");
        self.watch_error.get_or_insert(error);
        if self.retry_at.is_none() {
            self.retry_at = Some(Instant::now() + self.retry_delay);
        }
        self.publish_watch_status();
    }

    fn finish_watch_attempt(&mut self) {
        if self.watch_error.is_none() {
            self.retry_at = None;
            self.retry_delay = Duration::from_secs(1);
        }
        self.publish_watch_status();
    }

    fn request_rescan(&self, roots: Vec<AbsPathBuf>) {
        let mut roots: Vec<_> = roots
            .into_iter()
            .filter(|root| {
                self.watch_roots
                    .iter()
                    .any(|watched| root.starts_with(watched) || watched.starts_with(root))
            })
            .collect();
        if roots.is_empty() {
            roots.extend(self.watch_roots.iter().cloned());
        }
        roots.sort();
        roots.dedup();
        self.send(loader::Message::Rescan { config_version: self.config_version, roots });
    }

    fn retry_watches(&mut self) {
        self.retry_at = None;
        self.retry_delay = (self.retry_delay * 2).min(Duration::from_secs(30));
        let roots = std::mem::take(&mut self.watch_roots);
        self.watcher = None;
        self.watch_aliases.clear();
        self.watch_error = None;
        self.create_watcher();
        for root in roots {
            let path = root.as_ref() as &Path;
            if let Some(parent) = path.ancestors().find(|path| path.is_dir()) {
                self.watch(parent);
            } else {
                self.watch(path);
            }
        }
        self.finish_watch_attempt();
        // The fallback is finite and reads explicit inputs only. Directory
        // discovery goes through the project loader, preserving selection and
        // unsaved overlays. Backoff survives a reload after failed recovery.
        let files =
            self.watched_file_entries.iter().map(|path| (path.clone(), read(path))).collect();
        self.send(loader::Message::Changed { files });
        self.request_rescan(Vec::new());
    }

    fn watch(&mut self, path: &Path) {
        let Some(original) = Utf8PathBuf::from_path_buf(path.to_path_buf())
            .ok()
            .and_then(|p| AbsPathBuf::try_from(p).ok())
        else {
            return;
        };
        let original = original.normalize();
        self.watch_roots.insert(original.clone());
        let Some((watcher, _)) = &mut self.watcher else { return };
        if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
            self.watch_failed(format!("{}: {error}", path.display()));
            return;
        }
        // Only the native event boundary resolves symlinks. VFS identities
        // retain every subscribed spelling, including removed descendants.
        let Some(native) = fs::canonicalize(path)
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            .and_then(|p| AbsPathBuf::try_from(p).ok())
        else {
            return;
        };
        let alias = (native.normalize(), original);
        if alias.0 != alias.1 && !self.watch_aliases.contains(&alias) {
            self.watch_aliases.push(alias);
        }
    }

    #[track_caller]
    fn send(&self, msg: loader::Message) {
        self.sender.send(msg).unwrap();
    }
}

fn existing_watch_parent(path: &Path) -> Option<&Path> {
    path.parent()?.ancestors().find(|p| p.is_dir())
}

fn native_watcher(sender: Sender<NotifyEvent>) -> notify::Result<RecommendedWatcher> {
    RecommendedWatcher::new(
        move |event| {
            let _ = sender.send(event);
        },
        Config::default(),
    )
}

fn read(path: &AbsPath) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Is `path` a symlink to a parent directory?
///
/// Including this path is guaranteed to cause an infinite loop. This
/// heuristic is not sufficient to catch all symlink cycles (it's
/// possible to construct cycle using two or more symlinks), but it
/// catches common cases.
fn path_might_be_cyclic(path: &Path) -> bool {
    let Ok(destination) = std::fs::read_link(path) else {
        return false;
    };

    // If the symlink is of the form "../..", it's a parent symlink.
    let is_relative_parent =
        destination.components().all(|c| matches!(c, Component::CurDir | Component::ParentDir));

    is_relative_parent || path.starts_with(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loader::Handle;
    use std::time::{Duration, Instant};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "sail-watch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> AbsPathBuf {
            AbsPathBuf::assert_utf8(self.0.join(name))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ready(rx: &Receiver<loader::Message>) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if matches!(
                rx.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap(),
                loader::Message::Progress { n_done: LoadingProgress::Finished, .. }
            ) {
                return;
            }
        }
    }
    fn changed(rx: &Receiver<loader::Message>, path: &AbsPathBuf, expected: Option<&[u8]>) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let message = rx
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| panic!("no matching change for {path}: {expected:?}"));
            if let loader::Message::Changed { files } = message {
                if files.iter().any(|(p, bytes)| p == path && bytes.as_deref() == expected) {
                    return;
                }
            }
        }
    }

    /// Inject the canonical paths returned by FSEvents while retaining real
    /// subscriptions and filesystem reads. This reproduces alias mismatches on
    /// Unix hosts whose native backend normally preserves the subscribed path.
    #[cfg(unix)]
    struct CanonicalEvents {
        backend: Sender<NotifyEvent>,
        received: Receiver<loader::Message>,
        control: Sender<Message>,
        thread: std::thread::JoinHandle<()>,
        paths: Vec<AbsPathBuf>,
    }

    #[cfg(unix)]
    fn canonical_event_fixture(f: &Fixture, relative: &str, directories: bool) -> CanonicalEvents {
        let real = f.0.join("real");
        fs::create_dir_all(&real).unwrap();
        let (output, received) = unbounded();
        let (backend, events) = unbounded();
        let mut actor = NotifyActor::new(output);
        actor.watcher = Some((RecommendedWatcher::new(|_| {}, Config::default()).unwrap(), events));
        let mut paths = Vec::new();
        for name in ["first", "second"] {
            let link = f.path(name);
            std::os::unix::fs::symlink(&real, &link).unwrap();
            actor.watch(link.as_ref());
            let path = link.join(relative);
            if directories {
                actor.watched_dir_entries.push(loader::Directories {
                    extensions: vec!["sail".into()],
                    include: vec![link],
                    exclude: vec![],
                });
            } else {
                actor.watched_file_entries.insert(path.clone());
            }
            paths.push(path);
        }
        let (control, inbox) = unbounded();
        let thread = std::thread::spawn(move || actor.run(inbox));
        CanonicalEvents { backend, received, control, thread, paths }
    }

    #[cfg(unix)]
    fn assert_alias_changes(
        received: &Receiver<loader::Message>,
        paths: &[AbsPathBuf],
        expected: Option<&[u8]>,
    ) {
        let loader::Message::Changed { files } =
            received.recv_timeout(Duration::from_secs(10)).unwrap()
        else {
            panic!("expected native change")
        };
        assert_eq!(files.len(), paths.len(), "{files:?}");
        for path in paths {
            assert!(
                files.iter().any(|(p, bytes)| p == path && bytes.as_deref() == expected),
                "{path}: {files:?}"
            );
        }
    }

    #[test]
    fn watch_failures_coalesce_and_shutdown_interrupts_a_pending_retry() {
        let (output, received) = unbounded();
        let mut actor = NotifyActor::new(output);
        actor.watch_failed("native watch limit reached".into());
        let retry = actor.retry_at.unwrap();
        for _ in 0..100 {
            actor.watch_failed("another subscription failed".into());
        }
        assert_eq!(actor.retry_at, Some(retry), "errors must not keep postponing recovery");
        assert_eq!(received.try_iter().count(), 1, "report the transition, not every event");
        actor.retry_at = Some(Instant::now() + Duration::from_secs(3600));
        let (control, inbox) = unbounded();
        let (done, finished) = unbounded();
        let thread = std::thread::spawn(move || {
            actor.run(inbox);
            done.send(()).unwrap();
        });
        drop(control);
        finished.recv_timeout(Duration::from_secs(5)).expect("a retry timer cannot delay shutdown");
        thread.join().unwrap();
    }

    #[test]
    fn persistent_watch_failures_back_off_poll_only_explicit_inputs_and_recover() {
        let f = Fixture::new();
        let explicit = f.path("selected.sail");
        fs::write(&explicit, b"val selected : int").unwrap();
        fs::write(f.path("inactive.sail"), b"val inactive : bool").unwrap();
        let (output, received) = unbounded();
        let mut actor = NotifyActor::new(output);
        actor.watch_roots.insert(f.path(""));
        actor.watched_file_entries.insert(explicit.clone());
        actor.watcher_factory = |_| Err(notify::Error::generic("native watcher unavailable"));
        for attempt in 0..12 {
            actor.retry_watches();
            assert!(actor.retry_at.is_some());
            assert!(actor.watch_error.is_some());
            assert!(actor.retry_delay <= Duration::from_secs(30));
            if attempt >= 4 {
                assert_eq!(actor.retry_delay, Duration::from_secs(30));
            }
            let messages: Vec<_> = received.try_iter().collect();
            let files: Vec<_> = messages
                .iter()
                .filter_map(|m| {
                    if let loader::Message::Changed { files } = m {
                        Some(files)
                    } else {
                        None
                    }
                })
                .flatten()
                .collect();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].0, explicit);
            assert_eq!(files[0].1.as_deref(), Some(b"val selected : int".as_slice()));
            assert!(messages.iter().any(|m| matches!(m, loader::Message::Rescan { .. })));
        }
        fs::write(&explicit, b"val selected : bool").unwrap();
        actor.watcher_factory = native_watcher;
        actor.retry_watches();
        assert!(actor.retry_at.is_none());
        assert!(actor.watch_error.is_none());
        assert_eq!(actor.retry_delay, Duration::from_secs(1));
        let messages: Vec<_> = received.try_iter().collect();
        assert!(messages
            .iter()
            .any(|m| matches!(m, loader::Message::WatchStatus { error: None, .. })));
        assert!(messages.iter().any(|m| matches!(m, loader::Message::Changed { files } if files.iter().any(|(p, text)| p == &explicit && text.as_deref() == Some(b"val selected : bool")))));
    }

    #[cfg(unix)]
    #[test]
    fn lost_event_signals_request_discovery_in_the_subscribed_namespace() {
        let f = Fixture::new();
        let CanonicalEvents { backend, received, control, thread, .. } =
            canonical_event_fixture(&f, "new.sail", true);
        fs::write(f.0.join("real/inactive.sail"), b"val inactive : int").unwrap();
        let event = notify::Event::new(EventKind::Other)
            .set_flag(notify::event::Flag::Rescan)
            .add_path(fs::canonicalize(f.0.join("real")).unwrap());
        backend.send(Ok(event)).unwrap();
        let loader::Message::Rescan { config_version, mut roots } =
            received.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("lost events must request a project rescan")
        };
        roots.sort();
        assert_eq!(config_version, 0);
        assert_eq!(roots, vec![f.path("first"), f.path("second")]);
        assert!(
            received.try_recv().is_err(),
            "do not load inactive sources while requesting discovery"
        );
        drop(control);
        thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_backend_errors_recover_and_deliver_later_changes() {
        let f = Fixture::new();
        let CanonicalEvents { backend, received, control, thread, paths } =
            canonical_event_fixture(&f, "new.sail", true);
        backend.send(Err(notify::Error::generic("native event stream failed"))).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let mut degraded = false;
        loop {
            match received.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap() {
                loader::Message::WatchStatus { error: Some(error), .. } => {
                    assert!(error.contains("event stream failed"));
                    degraded = true;
                }
                loader::Message::WatchStatus { error: None, .. } => {
                    assert!(degraded);
                    break;
                }
                _ => {}
            }
        }
        fs::write(f.0.join("real/new.sail"), b"val recovered : int").unwrap();
        loop {
            if let loader::Message::Changed { files } =
                received.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap()
            {
                if paths.iter().all(|path| {
                    files.iter().any(|(p, bytes)| {
                        p == path && bytes.as_deref() == Some(b"val recovered : int")
                    })
                }) {
                    break;
                }
            }
        }
        drop(control);
        thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn canonical_events_recover_missing_descendants_in_each_subscribed_spelling() {
        let f = Fixture::new();
        let CanonicalEvents { backend, received, control, thread, paths } =
            canonical_event_fixture(&f, "missing/nested/lib.sail", false);
        let native = fs::canonicalize(f.0.join("real")).unwrap();
        for cycle in 0..3 {
            fs::create_dir_all(native.join("missing/nested")).unwrap();
            let text = format!("val recovered_{cycle} : int");
            fs::write(native.join("missing/nested/lib.sail"), &text).unwrap();
            backend
                .send(Ok(notify::Event::new(EventKind::Create(notify::event::CreateKind::Folder))
                    .add_path(native.join("missing"))))
                .unwrap();
            assert_alias_changes(&received, &paths, Some(text.as_bytes()));
            fs::remove_dir_all(native.join("missing")).unwrap();
            backend
                .send(Ok(notify::Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
                    .add_path(native.join("missing"))))
                .unwrap();
            assert_alias_changes(&received, &paths, None);
        }
        drop(control);
        thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn canonical_events_discover_directory_files_without_changing_vfs_identity() {
        let f = Fixture::new();
        let CanonicalEvents { backend, received, control, thread, paths } =
            canonical_event_fixture(&f, "new.sail", true);
        let native = fs::canonicalize(f.0.join("real")).unwrap().join("new.sail");
        fs::write(&native, b"val added : int").unwrap();
        backend
            .send(Ok(notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
                .add_path(native.clone())))
            .unwrap();
        assert_alias_changes(&received, &paths, Some(b"val added : int"));
        fs::remove_file(&native).unwrap();
        backend
            .send(Ok(notify::Event::new(EventKind::Remove(notify::event::RemoveKind::File))
                .add_path(native)))
            .unwrap();
        assert_alias_changes(&received, &paths, None);
        drop(control);
        thread.join().unwrap();
    }

    #[test]
    fn explicit_watches_recover_missing_paths_replacements_and_deleted_directories() {
        let f = Fixture::new();
        let path = f.path("missing/nested/lib.sail");
        let (tx, rx) = unbounded();
        let mut handle = NotifyHandle::spawn(tx);
        handle.set_config(loader::Config {
            version: 1,
            load: vec![loader::Entry::Files(vec![path.clone()])],
            watch: vec![0],
        });
        ready(&rx);
        fs::create_dir_all(f.0.join("missing/nested")).unwrap();
        fs::write(&path, b"val a : int").unwrap();
        changed(&rx, &path, Some(b"val a : int"));

        let replacement = f.path("replacement.tmp");
        fs::write(&replacement, b"val b : int").unwrap();
        // Windows rename cannot replace an existing destination. Both steps
        // must leave the parent watch alive on every platform.
        fs::remove_file(&path).unwrap();
        fs::rename(&replacement, &path).unwrap();
        changed(&rx, &path, Some(b"val b : int"));
        fs::write(&path, b"val c : bool").unwrap();
        changed(&rx, &path, Some(b"val c : bool"));
        for cycle in 0..8 {
            fs::remove_dir_all(f.0.join("missing")).unwrap();
            changed(&rx, &path, None);
            fs::create_dir_all(f.0.join("missing/nested")).unwrap();
            let recovered = format!("val recovered_{cycle} : int");
            fs::write(&path, recovered.as_bytes()).unwrap();
            changed(&rx, &path, Some(recovered.as_bytes()));
            // Recovery must leave a live subscription, not just perform one
            // successful read in response to the parent's creation event.
            let modified = format!("val modified_{cycle} : bool");
            fs::write(&path, modified.as_bytes()).unwrap();
            changed(&rx, &path, Some(modified.as_bytes()));
        }
    }

    #[test]
    fn watch_only_directories_do_not_load_unselected_sources() {
        let f = Fixture::new();
        fs::write(f.path("inactive.sail"), b"val unused : int").unwrap();
        let (tx, rx) = unbounded();
        let mut handle = NotifyHandle::spawn(tx);
        handle.set_config(loader::Config {
            version: 1,
            load: vec![loader::Entry::WatchDirectories(loader::Directories {
                extensions: vec!["sail".into()],
                include: vec![f.path("")],
                exclude: vec![],
            })],
            watch: vec![0],
        });
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap() {
                loader::Message::Loaded { files } => assert!(files.is_empty()),
                loader::Message::Progress { n_done: LoadingProgress::Finished, .. } => break,
                _ => {}
            }
        }
        let path = f.path("new.sail");
        fs::write(&path, b"val added : int").unwrap();
        changed(&rx, &path, Some(b"val added : int"));
    }

    // Linux's native backend emits open events, the fallback used to discover
    // files on FUSE. An editor opening an unlinked file must not turn each
    // project load into another synthetic creation event.
    #[cfg(target_os = "linux")]
    #[test]
    fn reconfiguration_does_not_rediscover_an_opened_unlinked_file() {
        let f = Fixture::new();
        let path = f.path("unlinked.sail");
        fs::write(&path, b"val unlinked : int").unwrap();
        let (tx, rx) = unbounded();
        let mut handle = NotifyHandle::spawn(tx);
        let config = |version| loader::Config {
            version,
            load: vec![loader::Entry::WatchDirectories(loader::Directories {
                extensions: vec!["sail".into()],
                include: vec![f.path("")],
                exclude: vec![],
            })],
            watch: vec![0],
        };
        handle.set_config(config(1));
        ready(&rx);
        fs::read(&path).unwrap();
        changed(&rx, &path, Some(b"val unlinked : int"));
        for version in 2..6 {
            handle.set_config(config(version));
            ready(&rx);
            fs::read(&path).unwrap();
            let until = Instant::now() + Duration::from_millis(100);
            while let Ok(message) = rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                assert!(
                    !matches!(message, loader::Message::Changed { .. }),
                    "reading an existing unlinked file triggered another change"
                );
            }
            // Real modifications must still be observed after every reset.
            let updated = format!("val unlinked_{version} : int");
            fs::write(&path, updated.as_bytes()).unwrap();
            changed(&rx, &path, Some(updated.as_bytes()));
        }
        fs::remove_file(&path).unwrap();
        changed(&rx, &path, None);
        fs::write(&path, b"val recreated : int").unwrap();
        changed(&rx, &path, Some(b"val recreated : int"));
    }
}
