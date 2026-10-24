//! The sixth probe: the server, driven as a client drives it.
//!
//! `corpus_check`, `conformance_check`, `mutation_check`, `analysis_stats`
//! and `protocol_check` all enter at `Analysis` with a file set they built
//! by walking a directory. Everything between the wire and that call —
//! the event loop, `MemDocs`, the workspace scan, eviction, when a
//! diagnostic is published and to whom — is below all five, and four
//! defects lived there at once while every probe stayed green:
//!
//! - eviction cancelled the diagnostics wave and nothing asked again, so an
//!   **opened file was published no diagnostics at all**;
//! - a rescan replaced every open buffer with what was last saved;
//! - `didClose` left the buffer's text in the database for the session;
//! - the scan published an empty list for every file without telling
//!   `DiagnosticCollection`, so the identical wave that followed compared
//!   equal, republished nothing, and the problems panel stayed empty.
//!
//! Each was found by hand, over stdio, against a release binary. This is
//! that harness in the test suite: `Connection::memory()`, the real
//! [`crate::main_loop::run_server`] on a thread, and a client that sends
//! the notifications an editor sends and reads what comes back.
//!
//! **`SAIL_DIR` points at an empty `lib/`**, so the bundled standard
//! library is neither materialised nor scanned and the fixtures are three
//! lines of Sail that need no prelude. That keeps a scenario near a second;
//! the corpus-sized question is `corpus_check`'s, not this one's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::Url;

/// `SAIL_DIR` is process-wide, so fixtures run one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How long a scenario waits for the server to answer before failing.
const PATIENCE: Duration = Duration::from_secs(60);

/// Whether the client reaches the workspace directly or through a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    Real,
    Symlinked,
    Detached,
}

pub(crate) struct ServerFixture {
    client: Connection,
    server: Option<std::thread::JoinHandle<()>>,
    /// What the client calls the workspace — a symlink, in one fixture.
    root: PathBuf,
    /// What to delete afterwards.
    base: PathBuf,
    next_id: i32,
    /// Latest published diagnostics per file, as an editor would hold them.
    published: HashMap<Url, Vec<lsp_types::Diagnostic>>,
    responses: HashMap<i32, Response>,
    log: Vec<String>,
    load_failed: bool,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ServerFixture {
    /// A workspace on disk with these `(relative path, contents)` files, a
    /// server running on it, and `initialize`/`initialized` exchanged.
    pub(crate) fn new(name: &str, files: &[(&str, &str)]) -> ServerFixture {
        Self::open(name, files, Root::Real)
    }

    /// Tests requiring complete semantic analysis declare a project explicitly.
    /// Listed Sail files form one module in the supplied order. Tests of
    /// discovery/unlinked buffers use `new` instead.
    fn new_project(name: &str, files: &[(&str, &str)]) -> ServerFixture {
        if files.iter().any(|(path, _)| crate::reload::is_project_file(std::path::Path::new(path)))
        {
            return Self::new(name, files);
        }
        let paths = files
            .iter()
            .filter(|(p, _)| p.ends_with(".sail"))
            .map(|(p, _)| *p)
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = format!("Main {{ files {paths} }}");
        let mut files = files.to_vec();
        files.push(("sail.proj", &manifest));
        Self::new(name, &files)
    }

    /// The workspace reached through a symlinked directory, which is
    /// rust-analyzer's `Project::with_root_dir_contains_symlink` — a fixture
    /// option there rather than a fix, because RA refuses to canonicalize at
    /// all: `AbsPath::canonicalize` *panics*, with "we explicitly do not
    /// provide canonicalization API, as that is almost always a wrong
    /// solution". The scan here walks with `follow_links(true)` while the
    /// client names the link, so whether the two agree on a file's identity
    /// is a question to ask rather than to answer with a normalisation pass.
    #[cfg(unix)]
    pub(crate) fn new_through_symlink(name: &str, files: &[(&str, &str)]) -> ServerFixture {
        Self::open(name, files, Root::Symlinked)
    }

    fn open(name: &str, files: &[(&str, &str)], kind: Root) -> ServerFixture {
        Self::open_with_imports(name, files, kind, None)
    }

    fn open_with_imports(
        name: &str,
        files: &[(&str, &str)],
        kind: Root,
        imports: Option<bool>,
    ) -> ServerFixture {
        let guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

        let base = std::env::temp_dir().join(format!("sail_lsp_server_check_{name}"));
        let _ = std::fs::remove_dir_all(&base);
        // `Root::Symlinked` puts the files in `<base>/real` and hands the
        // client `<base>/link`, so every path it sends crosses the link.
        let (root, contents_at) = match kind {
            Root::Real | Root::Detached => (base.clone(), base.clone()),
            Root::Symlinked => (base.join("link"), base.join("real")),
        };
        std::fs::create_dir_all(&contents_at).expect("workspace dir");
        #[cfg(unix)]
        if kind == Root::Symlinked {
            std::os::unix::fs::symlink(&contents_at, &root).expect("symlink");
        }
        for (rel, text) in files {
            let path = contents_at.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("parent dir");
            }
            std::fs::write(path, text).expect("fixture file");
        }
        // An empty library, so nothing but the fixture is loaded.
        let sail_dir = base.join("no-stdlib");
        std::fs::create_dir_all(sail_dir.join("lib")).expect("empty lib dir");
        // SAFETY: the mutex above makes this the only thread setting it.
        unsafe { std::env::set_var("SAIL_DIR", &sail_dir) };

        let (server_side, client) = Connection::memory();
        let server = std::thread::Builder::new()
            .name(format!("server_check-{name}"))
            .stack_size(32 * 1024 * 1024)
            .spawn(move || crate::main_loop::run_server(server_side))
            .expect("server thread");

        let mut fixture = ServerFixture {
            client,
            server: Some(server),
            root,
            base,
            next_id: 1,
            published: HashMap::new(),
            responses: HashMap::new(),
            log: Vec::new(),
            load_failed: false,
            _guard: guard,
        };
        fixture.initialize(imports, kind != Root::Detached);
        fixture
    }

    fn initialize(&mut self, imports: Option<bool>, has_workspace: bool) {
        let root_url = Url::from_directory_path(&self.root).expect("root url");
        let completion_properties = if imports.unwrap_or(false) {
            vec!["documentation", "additionalTextEdits"]
        } else {
            vec!["documentation"]
        };
        let id = self.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": has_workspace.then_some(&root_url),
                "workspaceFolders": has_workspace.then_some(serde_json::json!([{ "uri": root_url, "name": "fixture" }])),
                "capabilities": {
                    "window": { "workDoneProgress": true },
                    "textDocument": if imports.is_some() { serde_json::json!({
                        "completion": { "completionItem": { "resolveSupport": { "properties": completion_properties } } },
                        "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["quickfix"] } } }
                    }) } else { serde_json::json!({}) },
                    "workspace": {
                        "workspaceFolders": true,
                        // Dynamic registration, so the server does not start
                        // its own watcher: a scenario that wants the watcher
                        // path says so by not asking for this.
                        "didChangeWatchedFiles": { "dynamicRegistration": true },
                    },
                },
            }),
        );
        self.wait_for_response(id);
        self.notify("initialized", serde_json::json!({}));
        // The scan is asynchronous; its completion is what "ready" means.
        self.wait_until("workspace loading to finish", |f| {
            f.load_failed || f.log.iter().any(|m| m.contains("ready"))
        });
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> i32 {
        let id = self.next_id;
        self.next_id += 1;
        self.client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: method.to_owned(),
                params,
            }))
            .expect("send request");
        id
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.client
            .sender
            .send(Message::Notification(Notification { method: method.to_owned(), params }))
            .expect("send notification");
    }

    /// Read whatever has arrived, answering anything the server asks.
    fn pump(&mut self, block_for: Duration) {
        let deadline = Instant::now() + block_for;
        while let Ok(msg) =
            self.client.receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            match msg {
                Message::Notification(n) => match n.method.as_str() {
                    "textDocument/publishDiagnostics" => {
                        let params: lsp_types::PublishDiagnosticsParams =
                            serde_json::from_value(n.params).expect("publishDiagnostics params");
                        self.published.insert(params.uri, params.diagnostics);
                    }
                    "window/logMessage" => {
                        let params: lsp_types::LogMessageParams =
                            serde_json::from_value(n.params).expect("logMessage params");
                        self.load_failed |= params.typ == lsp_types::MessageType::ERROR;
                        self.log.push(params.message);
                    }
                    _ => {}
                },
                // A server request with no answer stalls whatever asked it.
                Message::Request(r) => {
                    let response =
                        Response { id: r.id, result: Some(serde_json::json!(null)), error: None };
                    self.client.sender.send(Message::Response(response)).ok();
                }
                Message::Response(r) => {
                    if let Some(id) = response_id(&r) {
                        self.responses.insert(id, r);
                    }
                }
            }
            if Instant::now() >= deadline {
                break;
            }
        }
    }

    /// Pump until `pred` holds, or fail with what the client had seen.
    fn wait_until(&mut self, what: &str, pred: impl Fn(&ServerFixture) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if pred(self) {
                return;
            }
            self.pump(Duration::from_millis(50));
        }
        panic!("waited {PATIENCE:?} for {what}; log so far: {:?}", self.log);
    }

    fn wait_for_response(&mut self, id: i32) -> Response {
        self.wait_until("a response", |f| f.responses.contains_key(&id));
        self.responses.remove(&id).expect("response")
    }

    pub(crate) fn url(&self, rel: &str) -> Url {
        Url::from_file_path(self.root.join(rel)).expect("file url")
    }

    pub(crate) fn did_open(&mut self, rel: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": self.url(rel), "languageId": "sail", "version": 1, "text": text,
            }}),
        );
    }

    /// An edit in the editor, as a range replacement — the incremental form
    /// every real client sends, and the one that goes through
    /// `DocumentData::apply_changes` and `from_proto::offset`.
    pub(crate) fn did_change_appending(&mut self, rel: &str, at_line: u32, text: &str) {
        self.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": self.url(rel), "version": 2 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": at_line, "character": 0 },
                        "end": { "line": at_line, "character": 0 },
                    },
                    "text": text,
                }],
            }),
        );
    }

    /// The buffer written to disk and `didSave` sent, as saving does.
    pub(crate) fn save(&mut self, rel: &str, text: &str) {
        std::fs::write(self.path(rel), text).expect("save");
        self.notify(
            "textDocument/didSave",
            serde_json::json!({ "textDocument": { "uri": self.url(rel) } }),
        );
    }

    pub(crate) fn did_close(&mut self, rel: &str) {
        self.notify(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": self.url(rel) } }),
        );
    }

    /// A file appearing on disk, as a client watcher reports it. Any `.sail`
    /// file created in the workspace makes the server rescan.
    pub(crate) fn create_on_disk(&mut self, rel: &str, text: &str) {
        std::fs::write(self.root.join(rel), text).expect("write");
        self.notify(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({ "changes": [{ "uri": self.url(rel), "type": 1 }] }),
        );
    }

    /// What the client currently shows for a file.
    pub(crate) fn shown(&self, rel: &str) -> Vec<lsp_types::Diagnostic> {
        self.published.get(&self.url(rel)).cloned().unwrap_or_default()
    }

    /// Pump until the file's published diagnostics satisfy `pred`.
    pub(crate) fn wait_for_diagnostics(
        &mut self,
        rel: &str,
        what: &str,
        pred: impl Fn(&[lsp_types::Diagnostic]) -> bool,
    ) {
        let url = self.url(rel);
        self.wait_until(what, |f| f.published.get(&url).is_some_and(|diags| pred(diags)));
    }

    /// Ask for something and wait for the answer, as JSON.
    pub(crate) fn ask(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        // A reload/edit can cancel a snapshot while the request is running.
        // Retry ServerCancelled as a client does; never turn protocol errors
        // into JSON null or retry an explicitly stale ContentModified action.
        for _ in 0..10 {
            let id = self.request(method, params.clone());
            let response = self.wait_for_response(id);
            if response
                .error
                .as_ref()
                .is_some_and(|error| error.code == lsp_server::ErrorCode::ServerCancelled as i32)
            {
                continue;
            }
            assert!(response.error.is_none(), "{method}: {response:?}");
            return response.result.unwrap_or(serde_json::Value::Null);
        }
        panic!("{method}: server cancelled ten consecutive requests");
    }

    /// What the server would answer a pull request with — which is the
    /// database's opinion, whether or not the file is open.
    pub(crate) fn pull(&mut self, rel: &str) -> Vec<lsp_types::Diagnostic> {
        // A pull can come back `ContentModified` — the write it raced with
        // is exactly what these scenarios provoke — and a client retries.
        let mut last_error = None;
        let mut result = None;
        for _ in 0..10 {
            let id = self.request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": self.url(rel) } }),
            );
            let response = self.wait_for_response(id);
            match response.result {
                Some(value) => {
                    result = Some(value);
                    break;
                }
                None => last_error = response.error,
            }
        }
        let report: lsp_types::DocumentDiagnosticReportResult = serde_json::from_value(
            result.unwrap_or_else(|| panic!("no diagnostic report; last error: {last_error:?}")),
        )
        .expect("diagnostic report shape");
        match report {
            lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Full(full),
            ) => full.full_document_diagnostic_report.items,
            _ => Vec::new(),
        }
    }

    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn outside_dir(&self) -> PathBuf {
        let mut dir = self.base.clone().into_os_string();
        dir.push("_outside");
        PathBuf::from(dir)
    }

    /// A document the workspace does not contain, opened from elsewhere.
    ///
    /// The distinction `clear_native_for` is written around: a workspace
    /// file keeps its diagnostics when it is closed, because that is what
    /// the problems panel is for, and a scratch file does not.
    pub(crate) fn open_outside_the_workspace(&mut self, name: &str, text: &str) -> Url {
        // A sibling of the workspace, not a child: "outside" is decided by
        // a prefix check against the workspace folder's URL.
        let outside = self.outside_dir();
        std::fs::create_dir_all(&outside).expect("outside dir");
        let path = outside.join(name);
        std::fs::write(&path, text).expect("scratch file");
        let uri = Url::from_file_path(&path).expect("scratch url");
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": uri, "languageId": "sail", "version": 1, "text": text,
            }}),
        );
        uri
    }

    pub(crate) fn close_url(&mut self, uri: &Url) {
        self.notify("textDocument/didClose", serde_json::json!({ "textDocument": { "uri": uri } }));
    }

    /// Pump until the file's published diagnostics are empty — which is a
    /// *published* empty list, not the absence of one.
    pub(crate) fn wait_for_diagnostics_cleared(&mut self, uri: &Url) {
        let uri = uri.clone();
        self.wait_until("the diagnostics to be cleared", |f| {
            f.published.get(&uri).is_some_and(|diags| diags.is_empty())
        });
    }

    pub(crate) fn shown_for(&self, uri: &Url) -> Vec<lsp_types::Diagnostic> {
        self.published.get(uri).cloned().unwrap_or_default()
    }

    /// Settings, as a client that cannot answer `workspace/configuration`
    /// sends them — which this fixture is, so `params.settings` is read
    /// rather than ignored.
    pub(crate) fn configure(&mut self, settings: serde_json::Value) {
        self.notify(
            "workspace/didChangeConfiguration",
            serde_json::json!({ "settings": settings }),
        );
    }

    /// A stand-in for `sail`, printing what upstream would print.
    ///
    /// rust-analyzer's flycheck slow-tests point `check.overrideCommand` at
    /// a real `rustc`, because a Rust toolchain is what CI is made of. A
    /// `sail` binary is not, and what this server owns is the invocation,
    /// the parse and the path to the panel — upstream's *wording* is held
    /// by `flycheck.rs`'s own tests against real output and by the
    /// `.expect` corpora.
    #[cfg(unix)]
    pub(crate) fn fake_checker(&self, prints: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.base.join("fake-sail");
        let script = format!("#!/bin/sh\ncat >&2 <<'UPSTREAM'\n{prints}\nUPSTREAM\nexit 1\n");
        std::fs::write(&path, script).expect("fake checker");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make it runnable");
        path
    }
}

impl Drop for ServerFixture {
    fn drop(&mut self) {
        let _ = self.client.sender.send(Message::Request(Request {
            id: RequestId::from(9999),
            method: "shutdown".to_owned(),
            params: serde_json::json!(null),
        }));
        self.notify("exit", serde_json::json!(null));
        // Dropping the client's channels ends the loop even if `exit` was
        // not reached, so the thread cannot outlive the test.
        drop(std::mem::replace(&mut self.client, Connection::memory().0));
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_dir_all(self.outside_dir());
        // The directory is gone, so leaving the variable set would point the
        // next reader at nothing. SAFETY: the fixture's mutex is still held.
        unsafe { std::env::remove_var("SAIL_DIR") };
    }
}

fn response_id(response: &Response) -> Option<i32> {
    serde_json::to_value(&response.id).ok()?.as_i64().map(|it| it as i32)
}

/// Something to report that needs no standard library.
const A_FILE_WITH_ONE_ERROR: &str = "\
default Order dec

function f(x : bits(8)) -> bits(8) = {
  let y : bits(4) = x;
  x
}
";

fn errors(diags: &[lsp_types::Diagnostic]) -> usize {
    diags.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count()
}

fn mentions(diags: &[lsp_types::Diagnostic], needle: &str) -> bool {
    diags.iter().any(|d| d.message.contains(needle))
}

/// Opening a file publishes its diagnostics.
///
/// The plainest thing a language server does, and it did not: eviction
/// cancelled the wave, a cancelled wave is dropped per file in
/// `fetch_native_diagnostics`, and eviction is not a change so nothing
/// asked again. On sail-riscv the editor showed nothing where
/// `corpus_check` reported a warning.
///
/// Checked by putting the defect back: with the shipped ordering — the
/// eviction below the wave, guarded on the results channel — this times out
/// with no diagnostics at all, exactly as the release binary did. What it
/// does *not* catch on a fixture this small is the second, unguarded call
/// site on its own, because priming here finishes before the wave starts;
/// `eviction_waits_for_the_pool_and_precedes_the_wave` holds that one by
/// counting call sites.
#[test]
fn failed_initial_load_stays_unlinked_until_explicit_configuration_recovers() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::new(
        "failed-initial-project",
        &[
            ("sail.proj", "Broken { files absent.sail }"),
            ("deep/model/selected.sail_project", "Main { files main.sail }"),
            ("deep/model/main.sail", source),
        ],
    );
    assert!(server.load_failed);
    server.did_open("deep/model/main.sail", source);
    let diagnostics = server.pull("deep/model/main.sail");
    assert_eq!(errors(&diagnostics), 0, "{diagnostics:?}");
    assert!(mentions(&diagnostics, "not part of a loaded Sail project"), "{diagnostics:?}");
    server.configure(
        serde_json::json!({"workspace": {"projectFile": "deep/model/selected.sail_project"}}),
    );
    server.wait_for_diagnostics(
        "deep/model/main.sail",
        "explicit project to restore type checking",
        |ds| errors(ds) == 1 && !mentions(ds, "not part of a loaded Sail project"),
    );
    assert!(server
        .log
        .iter()
        .any(|m| m.contains("Loaded Sail project:") && m.contains("selected.sail_project")));
}

#[test]
fn single_file_without_workspace_is_explicitly_unlinked() {
    let source = "function local(x : int) -> int = missing(x)\n";
    let mut server =
        ServerFixture::open("no-workspace", &[("scratch.sail", source)], Root::Detached);
    server.did_open("scratch.sail", source);
    let diagnostics = server.pull("scratch.sail");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].code,
        Some(lsp_types::NumberOrString::String("unlinked-file".into()))
    );
    let hover = server.ask(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": server.url("scratch.sail")},
            "position": {"line": 0, "character": 10}
        }),
    );
    assert!(hover.to_string().contains("function local"), "{hover}");
}

#[test]
fn unlinked_files_keep_local_features_and_follow_manifest_membership_changes() {
    let core = "function use(x : int) -> int = zero_extend(x)\nfunction bad() -> int = true\n";
    let prelude = "val zero_extend : int -> int\nfunction zero_extend(x) = x\n";
    let mut server = ServerFixture::new(
        "unlinked-membership",
        &[
            ("core.sail", core),
            ("prelude.sail", prelude),
            ("deep/nested/ignored.sail_project", "Unused { files missing.sail }"),
        ],
    );
    server.did_open("core.sail", core);
    server.did_open("prelude.sail", prelude);
    let unlinked = |ds: &[lsp_types::Diagnostic]| {
        ds.iter().any(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
    };
    let diagnostics = server.pull("core.sail");
    assert!(unlinked(&diagnostics), "{diagnostics:?}");
    assert_eq!(errors(&diagnostics), 0, "{diagnostics:?}");
    assert!(server.log.iter().any(|m| m.contains("No Sail project loaded")));
    let hover = server.ask(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": server.url("core.sail")},
            "position": {"line": 0, "character": 10}
        }),
    );
    assert!(hover.to_string().contains("function use"), "{hover}");
    let definition = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": server.url("core.sail")},
            "position": {"line": 0, "character": core.find("zero_extend").unwrap()}
        }),
    );
    assert!(!definition.to_string().contains("prelude.sail"), "{definition}");

    server.did_open("broken.sail", "function broken( =\n");
    let broken = server.pull("broken.sail");
    assert!(unlinked(&broken) && errors(&broken) > 0, "{broken:?}");

    std::fs::write(
        server.path("sail.proj"),
        "Prelude { files prelude.sail } Core { requires Prelude files core.sail }",
    )
    .unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": server.url("sail.proj"), "type": 1}]
        }),
    );
    server.wait_for_diagnostics("core.sail", "configured project diagnostics", |ds| {
        !unlinked(ds) && errors(ds) == 1
    });
    let configured = server.pull("core.sail");
    assert!(!unlinked(&configured) && errors(&configured) == 1, "{configured:?}");
    assert!(!mentions(&configured, "zero_extend"), "{configured:?}");
    assert!(server
        .log
        .iter()
        .any(|m| m.contains("Loaded Sail project:") && m.contains("sail.proj")));

    // Being next to a configured file does not grant its scope to a buffer.
    server.did_open("excluded.sail", core);
    let excluded = server.pull("excluded.sail");
    assert!(unlinked(&excluded) && errors(&excluded) == 0, "{excluded:?}");
    let definition = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": server.url("excluded.sail")},
            "position": {"line": 0, "character": core.find("zero_extend").unwrap()}
        }),
    );
    assert!(!definition.to_string().contains("prelude.sail"), "{definition}");

    let excluded_uri = server.url("excluded.sail");
    server.close_url(&excluded_uri);
    server.wait_for_diagnostics_cleared(&excluded_uri);

    std::fs::remove_file(server.path("sail.proj")).unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": server.url("sail.proj"), "type": 3}]
        }),
    );
    server.wait_for_diagnostics(
        "core.sail",
        "unlinked diagnostics after manifest deletion",
        unlinked,
    );
    assert_eq!(errors(&server.pull("core.sail")), 0);
}

#[test]
fn nested_project_loads_prelude_before_core() {
    let prelude = "val zero_extend : int -> int\nfunction zero_extend(x) = x\n";
    let core = "function creg2reg_idx(i : int) -> int = zero_extend(i)\n";
    let mut server = ServerFixture::new(
        "nested-project",
        &[
            ("model/riscv.sail_project", "prelude { files prelude/prelude.sail } core { requires prelude files core/types.sail }"),
            ("model/prelude/prelude.sail", prelude),
            ("model/core/types.sail", core),
            ("tests/unrelated.sail", "function unrelated() = missing()\n"),
        ],
    );
    server.did_open("model/core/types.sail", core);
    let diagnostics = server.pull("model/core/types.sail");
    assert_eq!(errors(&diagnostics), 0, "{diagnostics:?}");
    let definition = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": server.url("model/core/types.sail")},
            "position": {"line": 0, "character": 40}
        }),
    );
    assert!(definition.to_string().contains("prelude/prelude.sail"), "{definition}");
}

#[test]
fn declaration_hover_keeps_a_single_declaration_head() {
    for (source, name, expected) in [
        (
            "function creg2reg_idx(i : int) -> int = i\n",
            "creg2reg_idx",
            "function creg2reg_idx(i : int) -> int",
        ),
        ("register counter : int\n", "counter", "register counter : int"),
        ("mapping enc : int <-> int = { x <-> x }\n", "enc", "mapping enc : int <-> int"),
    ] {
        let mut server =
            ServerFixture::new_project("hover-declaration-head", &[("types.sail", source)]);
        server.did_open("types.sail", source);
        let hover = server.ask(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": server.url("types.sail")},
                "position": {"line": 0, "character": source.find(name).unwrap()}
            }),
        );
        let rendered = hover["contents"]["value"].as_str().expect("hover text");
        assert_eq!(rendered.lines().next(), Some(expected), "{hover}");
    }
}

#[test]
fn opening_a_file_publishes_its_diagnostics() {
    let mut server = ServerFixture::new_project("open", &[("a.sail", A_FILE_WITH_ONE_ERROR)]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the file's diagnostics", |d| errors(d) == 1);
    assert!(mentions(&server.shown("a.sail"), "bits(4)"));
}

#[test]
fn repeated_include_contexts_survive_editor_requests_and_edits() {
    let shared = "$ifdef SECOND\nprivate val second : unit -> bool\nfunction second() = helper_b()\n$else\nprivate val first : unit -> int\nfunction first() = helper_a()\n$endif\n";
    let a = "private val helper_a : unit -> int\nfunction helper_a() = 1\n$include \"shared.sail\"\nval use_a : unit -> int\nfunction use_a() = first()\n";
    let b = "$define SECOND\nprivate val helper_b : unit -> bool\nfunction helper_b() = true\n$include \"shared.sail\"\nval use_b : unit -> bool\nfunction use_b() = second()\n";
    let mut server = ServerFixture::new_project(
        "include-instances",
        &[
            ("sail.proj", "A { files a.sail } B { after A files b.sail }"),
            ("a.sail", a),
            ("b.sail", b),
            ("shared.sail", shared),
        ],
    );
    for (path, source) in [("a.sail", a), ("b.sail", b), ("shared.sail", shared)] {
        server.did_open(path, source);
        assert_eq!(errors(&server.pull(path)), 0, "{path}");
    }
    for (line, expected, absent) in [(2, "b.sail", "a.sail"), (5, "a.sail", "b.sail")] {
        let definitions = server
            .ask(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": {"uri": server.url("shared.sail")},
                    "position": {"line": line, "character": 22}
                }),
            )
            .to_string();
        assert!(definitions.contains(expected) && !definitions.contains(absent), "{definitions}");
    }
    let outline = server
        .ask(
            "textDocument/documentSymbol",
            serde_json::json!({
                "textDocument": {"uri": server.url("shared.sail")}
            }),
        )
        .to_string();
    assert!(outline.contains("first") && outline.contains("second"), "{outline}");
    let lenses = server.ask(
        "textDocument/codeLens",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")}
        }),
    );
    for name in ["first", "second"] {
        for (kind, title) in [("refs", "1 reference"), ("impls", "1 implementation")] {
            let lens = lenses
                .as_array()
                .unwrap()
                .iter()
                .find(|lens| lens["data"]["name"] == name && lens["data"]["kind"] == kind)
                .unwrap();
            let resolved = server.ask("codeLens/resolve", lens.clone());
            assert_eq!(resolved["command"]["title"], title, "{resolved}");
        }
    }
    let tokens = server.ask(
        "textDocument/semanticTokens/full",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")}
        }),
    );
    let decode = |tokens: &serde_json::Value| {
        let (mut line, mut column) = (0, 0);
        tokens["data"]
            .as_array()
            .unwrap()
            .chunks_exact(5)
            .map(|token| {
                let delta = token[0].as_u64().unwrap();
                line += delta;
                column = if delta == 0 {
                    column + token[1].as_u64().unwrap()
                } else {
                    token[1].as_u64().unwrap()
                };
                (
                    line,
                    column,
                    token[2].as_u64().unwrap(),
                    token[3].as_u64().unwrap(),
                    token[4].as_u64().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    let full = decode(&tokens);
    for line in [2, 5] {
        assert!(
            full.iter().any(|t| t.0 == line && t.1 == 9 && t.3 == 1 && t.4 & 2 != 0),
            "both active definitions must be highlighted: {full:?}"
        );
    }
    assert!(full.windows(2).all(|pair| (pair[0].0, pair[0].1) < (pair[1].0, pair[1].1)));
    let range = server.ask(
        "textDocument/semanticTokens/range",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")},
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 3, "character": 0}}
        }),
    );
    assert_eq!(
        decode(&range),
        full.iter().copied().filter(|t| (1..3).contains(&t.0)).collect::<Vec<_>>()
    );
    let completion = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")},
            "position": {"line": 2, "character": 27}
        }),
    );
    let completion_text = completion.to_string();
    assert!(
        completion_text.contains("helper_b") && !completion_text.contains("helper_a"),
        "{completion}"
    );
    let items = completion.as_array().or_else(|| completion["items"].as_array()).unwrap();
    let item = items.iter().find(|item| item["label"] == "helper_b").unwrap().clone();
    let resolved = server.ask("completionItem/resolve", item);
    assert_eq!(resolved["label"], "helper_b");
    let rename = server.ask(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": server.url("b.sail")},
            "position": {"line": 5, "character": 21}, "newName": "renamed_second"
        }),
    );
    let changes = rename["changes"].as_object().expect("rename edits");
    assert!(changes.contains_key(server.url("shared.sail").as_str()), "{rename}");
    assert!(changes.contains_key(server.url("b.sail").as_str()), "{rename}");
    assert!(!changes.contains_key(server.url("a.sail").as_str()), "{rename}");
    let shared_edits = changes[server.url("shared.sail").as_str()].as_array().unwrap();
    assert_eq!(shared_edits.len(), 2, "{rename}");
    assert!(shared_edits.iter().all(|e| e["range"]["start"]["line"].as_u64().unwrap() < 3));
    let references = server
        .ask(
            "textDocument/references",
            serde_json::json!({
                "textDocument": {"uri": server.url("shared.sail")},
                "position": {"line": 1, "character": 15}, "context": {"includeDeclaration": true}
            }),
        )
        .to_string();
    assert!(references.contains("b.sail") && !references.contains("a.sail"), "{references}");

    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail"), "version": 2},
            "contentChanges": [{"text": shared.replace("helper_b()", "1")}]
        }),
    );
    let diagnostics = server.pull("shared.sail");
    assert!(errors(&diagnostics) > 0, "second instance must be checked: {diagnostics:?}");
    assert!(diagnostics
        .iter()
        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
        .all(|d| d.range.start.line == 2));
    assert_eq!(errors(&server.pull("a.sail")), 0);

    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail"), "version": 3},
            "contentChanges": [{"text": shared}]
        }),
    );
    assert_eq!(errors(&server.pull("shared.sail")), 0);
}

#[test]
fn disjoint_include_contexts_do_not_create_false_cycles() {
    let a = "$ifndef SECOND\n$include \"b.sail\"\n$endif\n";
    let b = "$ifdef SECOND\n$include \"a.sail\"\n$endif\n";
    let mut server = ServerFixture::new_project(
        "include-context-cycles",
        &[
            ("sail.proj", "A { files first.sail } B { after A files second.sail }"),
            ("first.sail", "$include \"a.sail\"\n"),
            ("second.sail", "$define SECOND\n$include \"b.sail\"\n"),
            ("a.sail", a),
            ("b.sail", b),
        ],
    );
    for (path, source) in [("a.sail", a), ("b.sail", b)] {
        server.did_open(path, source);
        assert_eq!(errors(&server.pull(path)), 0, "physical edges cycle, semantic visits do not");
    }
}

#[test]
fn shared_token_lenses_merge_contexts_without_duplicate_locations() {
    let shared = "private val leaf : unit -> int\n";
    let a = "$include \"shared.sail\"\nfunction use_a() = leaf()\n";
    let b = "$include \"shared.sail\"\nfunction use_b() = leaf()\n";
    let mut server = ServerFixture::new_project(
        "shared-token-lenses",
        &[
            ("sail.proj", "A { files a.sail } B { after A files b.sail }"),
            ("a.sail", a),
            ("b.sail", b),
            ("shared.sail", shared),
        ],
    );
    for (path, source) in [("a.sail", a), ("b.sail", b), ("shared.sail", shared)] {
        server.did_open(path, source);
    }
    let lenses = server.ask(
        "textDocument/codeLens",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")}
        }),
    );
    let refs: Vec<_> =
        lenses.as_array().unwrap().iter().filter(|l| l["data"]["kind"] == "refs").collect();
    assert_eq!(refs.len(), 1, "one physical declaration must have one lens");
    let lens = refs[0].clone();
    let resolved = server.ask("codeLens/resolve", lens.clone());
    assert_eq!(resolved["command"]["title"], "2 references", "{resolved}");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("b.sail"), "version": 2},
            "contentChanges": [{"text": ""}]
        }),
    );
    let resolved = server.ask("codeLens/resolve", lens);
    assert_eq!(resolved["command"]["title"], "1 reference", "{resolved}");
}

#[test]
fn repeated_manifest_roots_keep_topological_module_owners() {
    let shared = "$ifdef NEXT\nprivate val second : unit -> bool\nfunction second() = true\n$else\nprivate val first : unit -> int\nfunction first() = 1\n$endif\n$define NEXT\n";
    let a = "val use_a : unit -> int\nfunction use_a() = first()\n";
    let b = "val use_b : unit -> bool\nfunction use_b() = second()\n";
    let mut server = ServerFixture::new_project(
        "repeated-roots",
        &[
            (
                "sail.proj",
                "B { after A files shared.sail, b.sail } A { files shared.sail, a.sail }",
            ),
            ("shared.sail", shared),
            ("a.sail", a),
            ("b.sail", b),
        ],
    );
    for (path, source) in [("shared.sail", shared), ("a.sail", a), ("b.sail", b)] {
        server.did_open(path, source);
        assert_eq!(errors(&server.pull(path)), 0, "{path}");
    }
    // No guard: the same physical function body really is declared twice.
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail"), "version": 2},
            "contentChanges": [{"text": "val repeated : unit -> int\nfunction repeated() = 1\n"}]
        }),
    );
    let diagnostics = server.pull("shared.sail");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("already bound") && d.message.contains("repeated")),
        "{diagnostics:?}"
    );
    let request = server.request(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail")},
            "position": {"line": 0, "character": 5}, "newName": "unsafe_rename"
        }),
    );
    let response = server.wait_for_response(request);
    assert!(
        response.error.as_ref().is_some_and(|e| e.message.contains("include contexts")),
        "{response:?}"
    );
    assert!(response.result.is_none());
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("shared.sail"), "version": 3},
            "contentChanges": [{"text": "type word = int\n"}]
        }),
    );
    let diagnostics = server.pull("shared.sail");
    assert!(
        diagnostics.iter().any(|d| d.message.contains("Cannot define type synonym word")),
        "{diagnostics:?}"
    );
}

#[test]
fn includes_follow_target_and_unsaved_edits_through_the_server() {
    let source = "$iftarget c\n$include <c.sail>\n$else\n$include <fallback.sail>\n$endif\nval main : unit -> int\nfunction main() = selected()\n$ifdef NEVER\n$include <missing.sail>\nenum hidden = { Bad, Bad }\nfunction inactive() = absent()\n$endif\n";
    let mut server = ServerFixture::new_project("include-closure", &[
        ("sail.proj", "Main { files main.sail }"),
        ("main.sail", source),
        ("no-stdlib/lib/c.sail", "val selected : unit -> int\nfunction selected() = 1\nval c_only : int\n"),
        ("no-stdlib/lib/fallback.sail", "val selected : unit -> bool\nfunction selected() = true\nval fallback_only : int\n"),
        ("no-stdlib/lib/new.sail", "val selected : unit -> int\nfunction selected() = 2\nval new_only : int\n"),
        ("no-stdlib/lib/unrelated.sail", "val unrelated_only : int\n"),
    ]);
    server.did_open("main.sail", source);
    server.wait_for_diagnostics("main.sail", "fallback return type mismatch", |d| errors(d) > 0);
    let symbols = server.ask("workspace/symbol", serde_json::json!({"query":"only"})).to_string();
    assert!(symbols.contains("fallback_only"), "{symbols}");
    assert!(!symbols.contains("c_only") && !symbols.contains("unrelated_only"), "{symbols}");

    server.log.clear();
    server.configure(serde_json::json!({"workspace": {"target": "c"}}));
    server.wait_until("target configuration reload", |s| s.log.iter().any(|l| l.contains("ready")));
    assert_eq!(errors(&server.pull("main.sail")), 0);
    let symbols = server.ask("workspace/symbol", serde_json::json!({"query":"only"})).to_string();
    assert!(symbols.contains("c_only") && !symbols.contains("fallback_only"), "{symbols}");

    server.log.clear();
    server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": server.url("main.sail"), "version": 2},
        "contentChanges": [{"text": "$include <new.sail>\nval main : unit -> int\nfunction main() = selected()\n"}]
    }));
    server.wait_until("unsaved include discovery", |s| s.log.iter().any(|l| l.contains("ready")));
    assert_eq!(errors(&server.pull("main.sail")), 0);
    let symbols = server.ask("workspace/symbol", serde_json::json!({"query":"only"})).to_string();
    assert!(symbols.contains("new_only") && !symbols.contains("c_only"), "{symbols}");
    server.log.clear();
    server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": server.url("main.sail"), "version": 3},
        "contentChanges": [{"text": "$include <missing-active.sail>\nval main : unit -> int\nfunction main() = selected()\n"}]
    }));
    server.wait_until("missing include reload", |s| s.log.iter().any(|l| l.contains("ready")));
    assert!(server
        .pull("main.sail")
        .iter()
        .any(|d| d.message.contains("unresolved $include")
            && d.message.contains("missing-active.sail")));
    server.log.clear();
    server.create_on_disk(
        "no-stdlib/lib/missing-active.sail",
        "val selected : unit -> int\nfunction selected() = 3\n",
    );
    server.wait_until("created include reload", |s| s.log.iter().any(|l| l.contains("ready")));
    assert_eq!(errors(&server.pull("main.sail")), 0);
}

/// An unsaved edit survives a rescan.
///
/// Creating any `.sail` file in the workspace makes the server rescan, and
/// the scan wrote every file's disk text into the database — replacing open
/// buffers with what was last saved.
#[test]
fn an_unsaved_edit_survives_a_rescan() {
    let mut server = ServerFixture::new_project("rescan", &[("a.sail", "default Order dec\n")]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the buffer's diagnostics", |d| errors(d) == 1);

    server.create_on_disk("b.sail", "default Order dec\n");
    server.wait_until("the rescan to settle", |f| {
        f.log.iter().filter(|m| m.contains("scan")).count() >= 2
    });

    // The buffer is still one edit ahead of the file, so its error stands.
    assert_eq!(
        errors(&server.pull("a.sail")),
        1,
        "the rescan replaced the open buffer with the file"
    );
}

/// A rescan does not empty the problems panel.
///
/// The scan published an empty list for every file "to clear stale state"
/// without telling `DiagnosticCollection`, so the wave that followed
/// computed the same findings, compared them equal, recorded no change and
/// published nothing. The client kept the empty list.
#[test]
fn a_rescan_leaves_the_problems_panel_alone() {
    let mut server = ServerFixture::new_project("panel", &[("a.sail", A_FILE_WITH_ONE_ERROR)]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the file's diagnostics", |d| errors(d) == 1);

    server.create_on_disk("b.sail", "default Order dec\n");
    server.wait_until("the rescan to settle", |f| {
        f.log.iter().filter(|m| m.contains("scan")).count() >= 2
    });
    // Give any late publish a chance to land before reading the panel.
    server.pump(Duration::from_millis(500));

    assert_eq!(
        errors(&server.shown("a.sail")),
        1,
        "the rescan left the client with an empty problems panel"
    );
}

/// Closing a document gives the file back.
///
/// `didClose` removed it from `MemDocs` and left its text in the database,
/// so a closed file kept reporting errors from text that exists nowhere —
/// and every other file resolved against it.
#[test]
fn closing_a_document_restores_the_file() {
    // The file has a finding of its own, so "restored" cannot be confused
    // with "read as empty" — which is what a failed read would produce, and
    // it reports nothing at all.
    let file_without_the_error =
        "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = {\n  let spare = x;\n  x\n}\n";
    let mut server = ServerFixture::new_project("close", &[("a.sail", file_without_the_error)]);

    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the buffer's diagnostics", |d| errors(d) == 1);

    server.did_close("a.sail");
    server.wait_until("the close to settle", |f| {
        let _ = f;
        true
    });
    server.pump(Duration::from_millis(200));

    let restored = server.pull("a.sail");
    assert_eq!(errors(&restored), 0, "the closed buffer is still what the database holds");
    assert!(
        mentions(&restored, "spare"),
        "the file's own finding is missing, so what was restored is not the file: {restored:?}"
    );
    assert!(server.path("a.sail").exists(), "the fixture file is still there");
}

/// Typing changes what is reported.
///
/// Every scenario above this one opens a file and never touches it again,
/// so the whole incremental path — `didChange`, `DocumentData::apply_changes`,
/// the re-inference, the second wave — was exercised by nothing here.
/// Checked by putting a defect in: a `didChange` that updates `MemDocs` and
/// not salsa leaves this waiting for an error that never arrives.
#[test]
fn an_edit_changes_what_is_reported() {
    let clean = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_project("edit", &[("a.sail", clean)]);
    server.did_open("a.sail", clean);
    server.wait_for_diagnostics("a.sail", "a clean file", |d| d.is_empty());

    // Append a function that does not typecheck.
    server.did_change_appending(
        "a.sail",
        3,
        "function g(x : bits(8)) -> bits(4) = {\n  let y : bits(4) = x;\n  y\n}\n",
    );
    server.wait_for_diagnostics("a.sail", "the edit's error", |d| errors(d) == 1);

    // And taking it back takes the diagnostic back.
    server.did_change_appending("a.sail", 3, "");
    server.pump(Duration::from_millis(300));
    let after = server.pull("a.sail");
    assert_eq!(errors(&after), 1, "the buffer still holds the edit; the pull must agree");
}

/// What is pushed and what is pulled are the same answer.
///
/// Two paths to the same `file_diagnostics`, and nothing compared them.
/// It mattered on the day the push path published nothing at all: pull was
/// still right, which is why VS Code's users would have seen diagnostics
/// and this repo's stdio harness saw none.
///
/// Not verified by injection — the two paths call the same function, so
/// making them disagree means writing a second implementation. It stands
/// as the assertion that nobody has.
#[test]
fn push_and_pull_agree() {
    let mut server = ServerFixture::new_project("agree", &[("a.sail", A_FILE_WITH_ONE_ERROR)]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the file's diagnostics", |d| errors(d) == 1);

    let pushed: Vec<String> = server.shown("a.sail").iter().map(|d| d.message.clone()).collect();
    let pulled: Vec<String> = server.pull("a.sail").iter().map(|d| d.message.clone()).collect();
    let (mut pushed, mut pulled) = (pushed, pulled);
    pushed.sort();
    pulled.sort();
    assert_eq!(pushed, pulled, "the pushed and pulled diagnostics differ");
}

/// A scratch file stops reporting when it is closed.
///
/// `clear_native_for` says in its own doc that it is "used when a file is
/// closed" and carried an `#[expect(dead_code)]` — nothing cleared
/// anything, so a file opened, diagnosed and closed left its errors in the
/// problems panel for the rest of the session. A workspace file keeps
/// them, which is rust-analyzer's rule and the useful one.
#[test]
fn closing_a_scratch_file_clears_its_diagnostics() {
    let mut server = ServerFixture::new_project("scratch", &[("a.sail", "default Order dec\n")]);
    let scratch = server.open_outside_the_workspace("scratch.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_until("the scratch file's diagnostics", |f| {
        f.shown_for(&scratch)
            .iter()
            .any(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
    });

    server.close_url(&scratch);
    server.wait_for_diagnostics_cleared(&scratch);
}

/// Saving does not change what an unedited buffer reports.
///
/// The save path exists for flycheck — `sail --just-check` re-reads whole
/// files from disk, so it runs on save and not on keystrokes — and nothing
/// here had ever sent a `didSave`. Deliberately weak while flycheck is off
/// by default: it says saving is not *destructive*, which is what the
/// handler can be wrong about. Turning flycheck on wants its own scenario.
#[test]
fn saving_leaves_the_diagnostics_alone() {
    let mut server = ServerFixture::new_project("save", &[("a.sail", A_FILE_WITH_ONE_ERROR)]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the file's diagnostics", |d| errors(d) == 1);

    server.save("a.sail", A_FILE_WITH_ONE_ERROR);
    server.pump(Duration::from_millis(500));
    assert_eq!(errors(&server.shown("a.sail")), 1, "saving changed what an unedited buffer says");
}

/// A workspace reached through a symlink behaves like the real one.
///
/// The scan walks with `follow_links(true)`; the client names the link. If
/// the two disagree about which file is which, an open buffer protects one
/// identity while a rescan overwrites the other — the exact shape of the
/// defects this module exists for. rust-analyzer asks this as a fixture
/// option (`with_root_dir_contains_symlink`) rather than answering it with
/// a normalisation pass: `paths::AbsPath::canonicalize` panics there, on
/// purpose.
///
/// The assertion *is* the identity check: every wait here is on the URL the
/// client used — the link path — so a server that published under the
/// resolved path would hang, which is what the editor would show. It
/// passes, so there is nothing to fix; the plan to canonicalise paths "in
/// one place" is closed by this test rather than by an argument.
#[cfg(unix)]
#[test]
fn a_symlinked_workspace_root_behaves_like_the_real_one() {
    let mut server = ServerFixture::new_through_symlink(
        "symlink",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", "default Order dec\n")],
    );
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_for_diagnostics("a.sail", "the buffer's diagnostics", |d| errors(d) == 1);

    // And the open buffer still outranks the file across the link.
    server.create_on_disk("b.sail", "default Order dec\n");
    server.wait_until("the rescan to settle", |f| {
        f.log.iter().filter(|m| m.contains("scan")).count() >= 2
    });
    assert_eq!(
        errors(&server.pull("a.sail")),
        1,
        "through a symlink, the rescan replaced the open buffer"
    );
}

/// What upstream said reaches the problems panel.
///
/// `flycheck.rs` runs `sail --just-check` on save and translates its output,
/// and this is the third source of diagnostics rust-analyzer has and this
/// server was missing: the classes the IDE provably cannot decide — `int`
/// where `range(0, 3)` is expected, and the mutants `mutation_check` still
/// misses — are exactly the ones upstream proves. Nothing drove it end to
/// end until now; the RA surface report named this gap by its RA counterpart,
/// `tests/slow-tests/flycheck`.
///
/// The checker is a stand-in printing upstream's own format. What is under
/// test is this server: that a saved file runs the configured command, that
/// its output is parsed and placed, that it reaches the client beside the
/// native diagnostics, and that a later run replaces it.
#[cfg(unix)]
#[test]
fn what_upstream_says_reaches_the_panel() {
    // Clean to this server's own analysis, so anything published is the
    // checker's.
    let clean = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_project("flycheck", &[("a.sail", clean)]);

    let checker = server.fake_checker(
        "Type error:\n\
         a.sail:3.10-11:\n\
         3 |function f(x : bits(8)) -> bits(8) = x\n  \
           |          ^\n  \
           | int(3) is not a subtype of range(0, 2)",
    );
    server.configure(serde_json::json!({
        "flycheck": { "enable": true, "command": checker.to_string_lossy() }
    }));

    server.did_open("a.sail", clean);
    server.wait_for_diagnostics("a.sail", "the file to be analysed", |d| d.is_empty());

    server.save("a.sail", clean);
    server.wait_for_diagnostics("a.sail", "what upstream said", |d| {
        d.iter().any(|it| it.source.as_deref() == Some("sail"))
    });

    let shown = server.shown("a.sail");
    let from_upstream = shown.iter().find(|d| d.source.as_deref() == Some("sail")).expect("it");
    assert!(
        from_upstream.message.contains("is not a subtype of"),
        "the checker's message did not survive the trip: {from_upstream:?}"
    );
    assert_eq!(from_upstream.range.start.line, 2, "upstream counts lines from 1, LSP from 0");

    // A run that finds nothing replaces what the last one found — upstream
    // stops at the first error in a file, so a merge would keep a finding it
    // no longer makes.
    let _quiet = server.fake_checker("");
    server.save("a.sail", clean);
    server.wait_for_diagnostics("a.sail", "the checker's finding to be withdrawn", |d| {
        !d.iter().any(|it| it.source.as_deref() == Some("sail"))
    });
}

/// A semantic-token delta describes the buffer as it is now.
///
/// The delta is diffed against a *cached baseline*, and that cache had no
/// invalidation at all: `invalidate` and `clear` were written, waived as
/// dead code, and never called. rust-analyzer calls them in exactly two
/// places — on `didClose`, and beside the refresh request that tells the
/// client its baselines are meaningless — and both are wired now.
///
/// What this scenario asks is the question underneath: after an edit, does
/// a delta still describe the text the client has? It does, and the reason
/// is worth recording rather than assuming — the tokens on the *new* side
/// are recomputed from the current snapshot every time, so the cache only
/// ever holds the old side of the diff. The waivers were memory hygiene,
/// not a wrong answer, and the path had no test either way.
#[test]
fn a_semantic_token_delta_describes_the_buffer_now() {
    let one_function = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_project("tokens", &[("a.sail", one_function)]);
    server.did_open("a.sail", one_function);

    let full = server.ask(
        "textDocument/semanticTokens/full",
        serde_json::json!({ "textDocument": { "uri": server.url("a.sail") } }),
    );
    let result_id =
        full.get("resultId").and_then(|v| v.as_str()).expect("a result id to diff against");
    let before = full.get("data").and_then(|v| v.as_array()).expect("tokens").len();
    assert!(before > 0, "no semantic tokens at all: {full}");

    // A second function, so the token stream genuinely grows.
    server.did_change_appending("a.sail", 3, "function g(y : bits(4)) -> bits(4) = y\n");
    server.pump(Duration::from_millis(300));

    let delta = server.ask(
        "textDocument/semanticTokens/full/delta",
        serde_json::json!({
            "textDocument": { "uri": server.url("a.sail") },
            "previousResultId": result_id,
        }),
    );
    // Either an edit list against the baseline, or a full answer — both are
    // protocol-legal; what matters is that it accounts for the new text.
    let tokens_now = match delta.get("edits") {
        Some(edits) => {
            let edits = edits.as_array().expect("edits");
            assert!(!edits.is_empty(), "the edit added a function and the delta says nothing");
            before
                + edits
                    .iter()
                    .map(|e| {
                        let inserted =
                            e.get("data").and_then(|d| d.as_array()).map_or(0, |d| d.len());
                        let deleted = e.get("deleteCount").and_then(|d| d.as_u64()).unwrap_or(0);
                        inserted - deleted as usize
                    })
                    .sum::<usize>()
        }
        None => delta.get("data").and_then(|v| v.as_array()).expect("full tokens").len(),
    };
    assert!(
        tokens_now > before,
        "the buffer gained a function; the delta leaves it with {tokens_now} tokens against {before}"
    );
}

/// A cancelled request is answered, and answered as cancelled.
///
/// `$/cancelRequest` names a request the client has given up on. The
/// protocol still requires an answer — a client that never gets one waits
/// for ever — and it must be `RequestCancelled`, not a result computed
/// after the fact. `req_queue.incoming.cancel` returns that response and
/// `GlobalState::cancel` sends it; nothing here had ever sent the
/// notification, and the queue existed for a while with no reader at all.
#[test]
fn a_cancelled_request_is_answered_as_cancelled() {
    let mut server = ServerFixture::new_project("cancel", &[("a.sail", A_FILE_WITH_ONE_ERROR)]);
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);

    // Ask something expensive enough to still be in flight, then take it back.
    let id = server.request(
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": server.url("a.sail") } }),
    );
    server.notify("$/cancelRequest", serde_json::json!({ "id": id }));

    let response = server.wait_for_response(id);
    // Racing the handler is legal: an answer that beat the cancellation is
    // an answer. What is not legal is silence, and `wait_for_response` is
    // what would have caught that.
    if let Some(error) = response.error {
        assert_eq!(
            error.code,
            lsp_server::ErrorCode::RequestCanceled as i32,
            "a cancelled request was answered with the wrong error: {error:?}"
        );
    }
}

/// A file deleted on disk stops being part of the workspace.
///
/// `didChangeWatchedFiles` with `DELETED` sets `needs_reload`, and the
/// rescan is what removes it — the scan is the only thing that decides
/// which files there are. Nothing exercised the delete path; the create
/// and change paths each have a scenario above.
#[test]
fn a_file_deleted_on_disk_leaves_the_workspace() {
    let clean = "default Order dec\n";
    let mut server = ServerFixture::new_project(
        "delete",
        &[("a.sail", clean), ("gone.sail", A_FILE_WITH_ONE_ERROR)],
    );
    server.wait_until("the workspace to hold both files", |f| {
        f.log.iter().any(|m| m.contains("2 files"))
    });

    std::fs::remove_file(server.path("gone.sail")).expect("delete");
    std::fs::write(server.path("sail.proj"), "Main { files a.sail }").unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({ "changes": [{ "uri": server.url("gone.sail"), "type": 3 }] }),
    );

    server.wait_until("the rescan to see one file", |f| {
        f.log.iter().any(|m| m.contains("background scan: 1 files"))
    });
}

/// The symbol index answers through the wire.
///
/// `workspace/symbol` is a salsa query keyed by the workspace's files, and
/// the scan's ordering and the loader's contents both feed it — which is
/// exactly what this session rebuilt underneath it. Asking for a name the
/// fixture declares is the cheapest end-to-end statement that the index is
/// built at all.
#[test]
fn the_workspace_symbol_index_answers() {
    let file = "default Order dec\n\nfunction findable_by_name(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_project("symbols", &[("a.sail", file)]);

    let found = server.ask("workspace/symbol", serde_json::json!({ "query": "findable" }));
    let names: Vec<String> = found
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.get("name").and_then(|n| n.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.iter().any(|n| n.contains("findable_by_name")),
        "the workspace index did not find a function it holds: {found}"
    );
}

/// The client's formatting options reach the formatter.
///
/// `Analysis::format_document` took `tab_size` and `insert_spaces` and
/// rebuilt the rest from `Default`, so `insertFinalNewline`,
/// `trimFinalNewlines` and `trimTrailingWhitespace` were parsed, cached for
/// code actions, and dropped — while *range* formatting, the handler
/// directly below it in the same file, passed all of them. "A feature's
/// config is a parameter, not a default" is a rule this project states, and
/// this is where it was broken, one function away from where it was kept.
///
/// `insertFinalNewline` is the one to ask about, and finding that out took
/// two wrong scenarios. The formatter is deliberately conservative, so
/// "misformatted input produces edits" is not a property it has; and
/// trailing whitespace is trimmed whether or not the client asks, because
/// that option reads `unwrap_or(true)`. A final newline is inserted only on
/// `Some(true)` — which is exactly the value that was being thrown away.
#[test]
fn the_clients_formatting_options_reach_the_formatter() {
    let no_final_newline = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x";
    let mut server = ServerFixture::new_project("format", &[("a.sail", no_final_newline)]);
    server.did_open("a.sail", no_final_newline);

    let edits = server.ask(
        "textDocument/formatting",
        serde_json::json!({
            "textDocument": { "uri": server.url("a.sail") },
            "options": { "tabSize": 2, "insertSpaces": true, "insertFinalNewline": true },
        }),
    );
    let edits = edits.as_array().cloned().unwrap_or_default();
    assert!(!edits.is_empty(), "asked to insert a final newline, the server found nothing to do");
    let new_text = edits[0].get("newText").and_then(|t| t.as_str()).expect("newText");
    assert!(
        new_text.ends_with('\n'),
        "the client asked for a final newline and did not get one: {new_text:?}"
    );

    // And what it produced is still the same program.
    server.save("a.sail", new_text);
    server.did_close("a.sail");
    server.pump(Duration::from_millis(300));
    assert_eq!(errors(&server.pull("a.sail")), 0, "the formatter's output does not typecheck");
}

/// Goto-definition follows a name into another file.
///
/// The third step of `goto_definition` — cross-file, by name, through the
/// symbol index — read the index off `Analysis::symbol_index`, a field
/// only `analysis_stats` ever sets. In the server it was always `None`, so
/// the step returned empty and navigation stopped at the file it was asked
/// in. The probe reported 99.7% while the product could not follow a name
/// out of a file, which is the same sentence as every other finding this
/// module exists for: a probe measures the path it enters by.
#[test]
fn goto_definition_follows_a_name_into_another_file() {
    let declares = "default Order dec\n\nfunction declared_elsewhere(x : bits(8)) -> bits(8) = x\n";
    let uses =
        "default Order dec\n\nfunction caller(y : bits(8)) -> bits(8) = declared_elsewhere(y)\n";
    let mut server =
        ServerFixture::new_project("goto", &[("decl.sail", declares), ("use.sail", uses)]);
    server.did_open("use.sail", uses);

    // The call, on the last line: `... = declared_elsewhere(y)`.
    let at = uses.lines().count() as u32 - 1;
    let character = uses.lines().last().unwrap().find("declared_elsewhere").unwrap() as u32 + 2;
    let answer = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": server.url("use.sail") },
            "position": { "line": at, "character": character },
        }),
    );
    let targets = match &answer {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(_) => vec![answer.clone()],
        _ => Vec::new(),
    };
    assert!(
        targets.iter().any(|t| t
            .get("uri")
            .or_else(|| t.get("targetUri"))
            .and_then(|u| u.as_str())
            .is_some_and(|u| u.ends_with("decl.sail"))),
        "goto-definition did not leave the file it was asked in: {answer}"
    );
}

/// A file with Windows line endings is analysed, not drowned.
///
/// Upstream's `ws` is space and tab (`lexer.mll:172`), so a bare `\r`
/// matches no rule and `sail` rejects a CRLF-saved file outright. The
/// decision here was not to follow it — "a user who opens a CRLF file wants
/// to be told once, by something that can offer to fix it, not by the lexer
/// sixty times" — and `cr_invisible` sits in `NOT_REPORTED` saying so. That
/// is a decision about *behaviour*, and nothing exercised it end to end:
/// what a client actually gets for such a file was written down and never
/// asked.
///
/// It gets what an editor should get — the file's real findings, at the
/// right lines, and nothing per line — which is also the assertion that
/// would fail if `\r` ever became a diagnostic.
#[test]
fn a_file_with_windows_line_endings_is_analysed() {
    let crlf = "default Order dec\r\n\r\nfunction f(x : bits(8)) -> bits(8) = {\r\n  let y : bits(4) = x;\r\n  x\r\n}\r\n";
    let mut server = ServerFixture::new_project("crlf", &[("a.sail", crlf)]);
    server.did_open("a.sail", crlf);
    server.wait_for_diagnostics("a.sail", "the file's diagnostics", |d| errors(d) == 1);

    let shown = server.shown("a.sail");
    assert!(
        shown.len() <= 3,
        "a CRLF file drew one diagnostic per line, which is the thing the \
         decision in `NOT_REPORTED` says not to do: {shown:?}"
    );
    let error = shown
        .iter()
        .find(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
        .expect("the type error");
    assert_eq!(
        error.range.start.line, 3,
        "the error is on the `let`, and `\\r` must not shift the line count: {error:?}"
    );
}

/// Completion answers, and answers with what the file declares.
///
/// The first request rust-analyzer marks `LatencySensitive`, and the one
/// this server had no wire test for at all: `completions()` is reached
/// through `handle_completion`, which is reached through the dispatcher's
/// retry path, and every part of that was exercised only by unit tests
/// entering at `ide-completion`. It is also the feature whose *live* engine
/// was found to be a different one from the engine twenty tests were
/// calling — which is what a wire test would have said in one line.
///
/// It turned up a defect on the way in, since fixed: a one-expression
/// function body (`= a_dec|`) answered *nothing*, because `analyze`
/// decided whether the cursor was at the top level of the file by counting
/// braces — and that body is at brace depth 0, so a position where a name
/// is most obviously wanted was read as an item list. The same text inside
/// a block answered, which is how it stayed hidden. Both shapes are asked
/// here now.
#[test]
fn completion_offers_what_the_file_declares() {
    let file = "default Order dec\n\nfunction a_declared_name(x : bits(8)) -> bits(8) = x\n\nfunction in_a_block(y : bits(8)) -> bits(8) = {\n  a_dec\n}\n\nfunction one_expression(y : bits(8)) -> bits(8) = a_dec\n";
    let mut server = ServerFixture::new_project("completion", &[("a.sail", file)]);
    server.did_open("a.sail", file);

    // At the end of the partial name on the last line.
    let line = 5;
    let character = 7;
    let answer = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": { "uri": server.url("a.sail") },
            "position": { "line": line, "character": character },
        }),
    );
    let items = match &answer {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(o) => {
            o.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default()
        }
        _ => Vec::new(),
    };
    assert!(
        items
            .iter()
            .filter_map(|it| it.get("label").and_then(|l| l.as_str()))
            .any(|label| label.contains("a_declared_name")),
        "completion in a block did not offer a function declared above it: {answer}"
    );

    // And the one-expression body, which answered nothing at all.
    let answer = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": { "uri": server.url("a.sail") },
            "position": { "line": 8, "character": 51 },
        }),
    );
    let items = match &answer {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(o) => {
            o.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default()
        }
        _ => Vec::new(),
    };
    assert!(
        items
            .iter()
            .filter_map(|it| it.get("label").and_then(|l| l.as_str()))
            .any(|label| label.contains("a_declared_name")),
        "completion in a one-expression body offered nothing: {answer}"
    );
}

#[test]
fn scattered_order_follows_include_expansion_and_reports_physical_sources() {
    let main = "scattered union U\n$include \"clause.sail\"\nend U\n";
    let clause = "union clause U = C : int\n";
    let mut server = ServerFixture::new_project(
        "scattered-order",
        &[("sail.proj", "M { files main.sail }"), ("main.sail", main), ("clause.sail", clause)],
    );
    server.did_open("main.sail", main);
    server.did_open("clause.sail", clause);
    assert_eq!(errors(&server.pull("main.sail")), 0, "an included clause fills its parent's head");
    assert_eq!(errors(&server.pull("clause.sail")), 0);
    server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": server.url("main.sail"), "version": 2},
        "contentChanges": [{"text": "scattered union U\nunion clause U = A : bool\nend U\n$include \"clause.sail\"\n"}]
    }));
    let diagnostics = server.pull("clause.sail");
    let error = diagnostics
        .iter()
        .find(|d| d.message.contains("already been ended"))
        .expect("cross-file clause after end");
    assert_eq!(error.range.start.line, 0);
    assert!(error
        .related_information
        .as_ref()
        .unwrap()
        .iter()
        .any(|r| r.location.uri == server.url("main.sail")));
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail"), "version": 3},
            "contentChanges": [{"text": main}]
        }),
    );
    assert_eq!(errors(&server.pull("main.sail")), 0);
    assert_eq!(errors(&server.pull("clause.sail")), 0);
}

#[test]
fn call_hierarchy_retains_origin_and_uses_each_files_line_index() {
    let declaration = "// long comment makes the target offset unlike the caller\n\n\nprivate val helper : unit -> int\n";
    let caller = "val run : unit -> int\nfunction run() = helper()\n";
    let other =
        "private val helper : unit -> int\nval run : unit -> int\nfunction run() = helper()\n";
    let mut server = ServerFixture::new_project(
        "call-identity",
        &[
            ("sail.proj", "A { files decl.sail, caller.sail } B { after A files other.sail }"),
            ("decl.sail", declaration),
            ("caller.sail", caller),
            ("other.sail", other),
        ],
    );
    server.did_open("caller.sail", caller);
    let prepared = server.ask(
        "textDocument/prepareCallHierarchy",
        serde_json::json!({
            "textDocument": {"uri": server.url("caller.sail")},
            "position": {"line": 1, "character": 18}
        }),
    );
    let target = prepared[0].clone();
    assert_eq!(target["uri"], server.url("decl.sail").as_str());
    assert_eq!(target["selectionRange"]["start"]["line"], 3);
    assert!(target["data"]["context"].is_number());
    let incoming = server.ask("callHierarchy/incomingCalls", serde_json::json!({"item": target}));
    assert_eq!(incoming.as_array().unwrap().len(), 1, "{incoming}");
    let from = incoming[0]["from"].clone();
    assert_eq!(from["uri"], server.url("caller.sail").as_str());
    assert_eq!(incoming[0]["fromRanges"][0]["start"]["line"], 1);
    assert_eq!(incoming[0]["fromRanges"][0]["start"]["character"], 17);
    let outgoing = server.ask("callHierarchy/outgoingCalls", serde_json::json!({"item": from}));
    assert_eq!(outgoing.as_array().unwrap().len(), 1, "{outgoing}");
    assert_eq!(outgoing[0]["to"]["uri"], server.url("decl.sail").as_str());
    assert_eq!(outgoing[0]["to"]["selectionRange"]["start"]["line"], 3);
    assert_eq!(outgoing[0]["fromRanges"][0]["start"]["line"], 1);
}

#[test]
fn generated_call_hierarchy_preserves_names_ranges_and_stale_continuations() {
    let defs = "// Generated functions share one source declaration.\n\ndefault Order dec\nbitfield B : bits(8) = { part : 3 .. 0 }\n";
    let caller = "val run : unit -> bits(8)\nfunction run() = _get_B_bits(Mk_B(0x00))\n";
    let mut server = ServerFixture::new_project(
        "generated-call-identity",
        &[
            ("sail.proj", "M { files defs.sail, caller.sail }"),
            ("defs.sail", defs),
            ("caller.sail", caller),
        ],
    );
    server.did_open("caller.sail", caller);
    server.did_open("defs.sail", defs);
    let prepared = server.ask(
        "textDocument/prepareCallHierarchy",
        serde_json::json!({
            "textDocument": {"uri": server.url("caller.sail")},
            "position": {"line": 1, "character": 9}
        }),
    );
    let outgoing =
        server.ask("callHierarchy/outgoingCalls", serde_json::json!({"item": prepared[0]}));
    assert_eq!(outgoing.as_array().unwrap().len(), 2, "{outgoing}");
    let mut saved = Vec::new();
    for name in ["_get_B_bits", "Mk_B"] {
        let edge = outgoing
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["to"]["name"] == name)
            .expect("distinct generated callee");
        let target = edge["to"].clone();
        assert_eq!(target["uri"], server.url("defs.sail").as_str());
        assert_eq!(target["selectionRange"]["start"]["line"], 3);
        assert_eq!(target["selectionRange"]["start"]["character"], 9);
        assert_eq!(target["data"]["name"], name);
        assert_eq!(target["data"]["generated"], true);
        let column = caller.lines().nth(1).unwrap().find(name).unwrap();
        assert_eq!(edge["fromRanges"][0]["start"]["character"], column);
        let incoming =
            server.ask("callHierarchy/incomingCalls", serde_json::json!({"item": target}));
        assert_eq!(incoming.as_array().unwrap().len(), 1, "{incoming}");
        assert_eq!(incoming[0]["from"]["name"], "run");
        assert_eq!(incoming[0]["from"]["uri"], server.url("caller.sail").as_str());
        assert_eq!(incoming[0]["fromRanges"][0]["start"]["line"], 1);
        assert_eq!(incoming[0]["fromRanges"][0]["start"]["character"], column);
        let empty = server.ask("callHierarchy/outgoingCalls", serde_json::json!({"item": target}));
        assert!(empty.is_null(), "{empty}");
        saved.push(target);
    }
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("defs.sail"), "version": 2},
            "contentChanges": [{"text": "\n"}]
        }),
    );
    for target in saved {
        for method in ["callHierarchy/incomingCalls", "callHierarchy/outgoingCalls"] {
            let result = server.ask(method, serde_json::json!({"item": target}));
            assert!(result.is_null(), "{result}");
        }
    }
}

#[test]
fn type_hierarchy_keeps_include_context_through_continuations() {
    let a = "// header\nprivate struct BaseA = { x : int }\n$include \"shared.sail\"\n";
    let b = "private struct BaseB = { x : bool }\n$define SECOND\n$include \"shared.sail\"\n";
    let shared = "$ifdef SECOND\ntype AliasB = BaseB\n$else\ntype AliasA = BaseA\n$endif\n";
    let mut server = ServerFixture::new_project(
        "type-identity",
        &[
            ("sail.proj", "A { files a.sail } B { after A files b.sail }"),
            ("a.sail", a),
            ("b.sail", b),
            ("shared.sail", shared),
        ],
    );
    server.did_open("shared.sail", shared);
    let mut contexts = Vec::new();
    for (line, parent, parent_line) in [(3, "a.sail", 1), (1, "b.sail", 0)] {
        let prepared = server.ask(
            "textDocument/prepareTypeHierarchy",
            serde_json::json!({
                "textDocument": {"uri": server.url("shared.sail")},
                "position": {"line": line, "character": 6}
            }),
        );
        let alias = prepared[0].clone();
        contexts.push(alias["data"]["context"].clone());
        let supers = server.ask("typeHierarchy/supertypes", serde_json::json!({"item": alias}));
        assert_eq!(supers.as_array().unwrap().len(), 1, "{supers}");
        assert_eq!(supers[0]["uri"], server.url(parent).as_str());
        assert_eq!(supers[0]["selectionRange"]["start"]["line"], parent_line);
        let subs = server.ask("typeHierarchy/subtypes", serde_json::json!({"item": supers[0]}));
        assert_eq!(subs.as_array().unwrap().len(), 1, "{subs}");
        assert_eq!(subs[0]["uri"], server.url("shared.sail").as_str());
        assert_eq!(subs[0]["selectionRange"]["start"]["line"], line);
        assert_eq!(subs[0]["data"], alias["data"]);
    }
    assert_ne!(contexts[0], contexts[1]);
}

#[test]
fn auto_include_round_trip_applies_real_edits_and_respects_client_capabilities() {
    let main = "// 🐚 source\r\n$ifdef CATALOG\r\n$include \"../lib/helper.sail\"\r\n$endif\r\nlet marker : string = \"🐚\" function run() -> unit = helper()\r\n";
    for imports in [false, true] {
        let mut server = ServerFixture::open_with_imports(
            if imports { "auto-include-resolve" } else { "auto-include-quickfix" },
            &[
                ("sail.proj", "Main { files src/main.sail }"),
                ("src/main.sail", main),
                ("lib/helper.sail", "/// helper docs\nval helper : unit -> unit\n"),
            ],
            Root::Real,
            Some(imports),
        );
        server.did_open("src/main.sail", main);
        let uri = server.url("src/main.sail");
        let index = ide_db::line_index::LineIndex::new(main);
        let offset = main.rfind("helper()").unwrap();
        let position = crate::to_proto::position_in(&index, offset);
        let actions = server.ask("textDocument/codeAction", serde_json::json!({
            "textDocument": {"uri": uri}, "range": {"start": position, "end": position}, "context": {"diagnostics": []}
        }));
        let action = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"].as_str().is_some_and(|s| s.starts_with("Add `$include")))
            .unwrap_or_else(|| panic!("{actions}"));
        let resolved_action = server.ask("codeAction/resolve", action.clone());
        let quick_edits = resolved_action["edit"]["changes"][uri.as_str()]
            .as_array()
            .unwrap_or_else(|| panic!("{resolved_action}"));
        assert_eq!(quick_edits.len(), 1);
        let quick: lsp_types::TextEdit = serde_json::from_value(quick_edits[0].clone()).unwrap();
        assert_eq!(quick.new_text, "\r\n$include \"../lib/helper.sail\"\r\n");
        let completions = server.ask("textDocument/completion", serde_json::json!({
            "textDocument": {"uri": uri}, "position": crate::to_proto::position_in(&index, offset + 3)
        }));
        let candidates: Vec<_> = completions
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["data"]["autoInclude"].is_object())
            .collect();
        assert_eq!(candidates.len(), usize::from(imports), "{completions}");
        if let Some(candidate) = candidates.first() {
            assert!(candidate.get("additionalTextEdits").is_none());
            let resolved = server.ask("completionItem/resolve", (*candidate).clone());
            let edits: Vec<lsp_types::TextEdit> =
                serde_json::from_value(resolved["additionalTextEdits"].clone()).unwrap();
            assert_eq!(edits, vec![quick.clone()]);
            assert!(resolved["documentation"].to_string().contains("helper docs"));
        }
        let at = crate::from_proto::offset(&index, quick.range.start);
        let mut updated = main.to_owned();
        updated.insert_str(at, &quick.new_text);
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": updated}]
            }),
        );
        let stale_action = server.request("codeAction/resolve", action.clone());
        assert_eq!(
            server.wait_for_response(stale_action).error.unwrap().code,
            lsp_server::ErrorCode::ContentModified as i32
        );
        if let Some(candidate) = candidates.first() {
            let stale_completion = server.request("completionItem/resolve", (*candidate).clone());
            assert_eq!(
                server.wait_for_response(stale_completion).error.unwrap().code,
                lsp_server::ErrorCode::ContentModified as i32
            );
        }
        let diagnostics = server.pull("src/main.sail");
        assert!(
            !diagnostics.iter().any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)),
            "{diagnostics:?}"
        );
    }
}
