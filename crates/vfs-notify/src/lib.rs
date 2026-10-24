//! An implementation of `loader::Handle`, based on `walkdir` and `notify`.
//!
//! Native subscriptions cover explicit dependencies, including missing files
//! outside workspace folders. Parent watches and reads after registration
//! preserve updates across file replacement and directory recreation.

use std::{
    fs,
    path::{Component, Path},
    sync::atomic::AtomicUsize,
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

struct NotifyActor {
    sender: loader::Sender,
    watched_file_entries: FxHashSet<AbsPathBuf>,
    watched_dir_entries: Vec<loader::Directories>,
    seen_paths: FxHashSet<AbsPathBuf>,
    // Drop order is significant.
    watcher: Option<(RecommendedWatcher, Receiver<NotifyEvent>)>,
}

#[derive(Debug)]
enum Event {
    Message(Message),
    NotifyEvent(NotifyEvent),
}

impl NotifyActor {
    fn new(sender: loader::Sender) -> NotifyActor {
        NotifyActor {
            sender,
            watched_dir_entries: Vec::new(),
            watched_file_entries: FxHashSet::default(),
            seen_paths: FxHashSet::default(),
            watcher: None,
        }
    }

    fn next_event(&self, receiver: &Receiver<Message>) -> Option<Event> {
        let Some((_, watcher_receiver)) = &self.watcher else {
            return receiver.recv().ok().map(Event::Message);
        };

        select! {
            recv(receiver) -> it => it.ok().map(Event::Message),
            recv(watcher_receiver) -> it => Some(Event::NotifyEvent(it.unwrap())),
        }
    }

    fn run(mut self, inbox: Receiver<Message>) {
        while let Some(event) = self.next_event(&inbox) {
            tracing::debug!(?event, "vfs-notify event");
            match event {
                Event::Message(msg) => match msg {
                    Message::Config(config) => {
                        self.watcher = None;
                        if !config.watch.is_empty() {
                            let (watcher_sender, watcher_receiver) = unbounded();
                            let watcher = log_notify_error(RecommendedWatcher::new(
                                move |event| {
                                    // we don't care about the error. If sending fails that usually
                                    // means we were dropped, so unwrapping will just add to the
                                    // panic noise.
                                    _ = watcher_sender.send(event);
                                },
                                Config::default(),
                            ));
                            self.watcher = watcher.map(|it| (it, watcher_receiver));
                        }

                        let config_version = config.version;

                        let n_total = config.load.len();
                        self.watched_dir_entries.clear();
                        self.watched_file_entries.clear();
                        self.seen_paths.clear();

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
                        for path in watch_roots {
                            self.watch(&path);
                        }
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
                Event::NotifyEvent(event) => {
                    if let Some(event) = log_notify_error(event) {
                        if matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
                                | EventKind::Access(AccessKind::Open(_))
                        ) {
                            let mut abs_paths: Vec<AbsPathBuf> = event
                                .paths
                                .into_iter()
                                .filter_map(|path| {
                                    let abs = AbsPathBuf::try_from(
                                        Utf8PathBuf::from_path_buf(path).ok()?,
                                    )
                                    .expect("path is absolute");
                                    // Normalize paths from notify events (resolve `.` / `..`).
                                    Some(abs.normalize())
                                })
                                .collect();

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

                            let mut saw_new_file = false;
                            for abs_path in &abs_paths {
                                if self.seen_paths.insert(abs_path.clone()) {
                                    saw_new_file = true;
                                }
                            }

                            // Only consider access events for files that we haven't seen
                            // before.
                            //
                            // This is important on FUSE filesystems, where we may not get a
                            // Create event. In other cases we're about to access the file, so
                            // we don't want an infinite loop where processing an Access event
                            // creates another Access event.
                            if matches!(event.kind, EventKind::Access(_)) && !saw_new_file {
                                continue;
                            }

                            let files = abs_paths
                                .into_iter()
                                .filter_map(|path| -> Option<(AbsPathBuf, Option<Vec<u8>>)> {
                                    // Ignore events for files/directories that we're not watching.
                                    if !(self.watched_file_entries.contains(&path)
                                        || self
                                            .watched_dir_entries
                                            .iter()
                                            .any(|dir| dir.contains_file(&path)))
                                    {
                                        return None;
                                    }

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

    fn watch(&mut self, path: &Path) {
        if let Some((watcher, _)) = &mut self.watcher {
            log_notify_error(watcher.watch(path, RecursiveMode::Recursive));
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

fn read(path: &AbsPath) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn log_notify_error<T>(res: notify::Result<T>) -> Option<T> {
    res.map_err(|err| tracing::warn!("notify error: {}", err)).ok()
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
}
