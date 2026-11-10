//! Exercise the real event loop with an in-memory LSP client.
//!
//! Fixtures cover workspace loading, editor overlays, diagnostics, cancellation
//! and request/response behavior through `run_server`. `SAIL_DIR` points to an
//! empty library so small scenarios do not load the bundled prelude. Large-model
//! and external-compiler checks have separate corpus/stdio gates.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::Url;

/// `SAIL_DIR` is process-wide, so fixtures run one at a time.
pub(crate) static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn escaped_file_uri_preserves_project_tasks_and_navigation() {
    let mut server = ServerFixture::new(
        "escaped-project-file-uri",
        &[("sail.proj", "Main { files main.sail }\n"), ("main.sail", "val answer : int\n")],
    );
    server.did_open("main.sail", "val answer : int\n");
    server.pull("main.sail");
    let normal = server.url("main.sail").to_string();
    let escaped = normal.replace("main.sail", "%6dain.sail");
    let params = serde_json::json!({"textDocument":{"uri":escaped}});
    let tasks = server.ask("experimental/runnables", params.clone());
    assert_eq!(tasks.as_array().unwrap().len(), 1, "{tasks}");
    let manifest = server.ask("sail-lsp/openProjectManifest", params.clone());
    assert_eq!(manifest["uri"], server.url("sail.proj").to_string());
    let parents = server.ask("sail-lsp/parentModule", params);
    assert_eq!(parents.as_array().unwrap().len(), 1, "{parents}");
}

#[test]
fn project_runnables_use_loaded_overrides_and_revalidate_manifest_and_task_changes() {
    use serde_json::json;
    let settings = |adapter: &str| {
        json!({
            "workspace": {
                "projectVariables": {"MODE":"global"}, "projectModules":["NotARoot"],
                "linkedProjects":[
                    {"projectFile":"a.sail_project","projectVariables":{"MODE":"A value"},"projectModules":["A"]},
                    {"projectFile":"b.sail_project","projectVariables":{"MODE":"B value"},"projectModules":["B"]}
                ]
            },
            "flycheck":{"enable":false,"command":"/tool chain/chosen-sail"},
            "runnables":{"tasks":[
                {"label":"Export C","kind":"export","project":"a.sail_project","command":"${sail}","args":["-c","${projectArgs}"]},
            {"label":"Suite","kind":"test","command":adapter,"args":["${projectContext}"],"environment":{"API_TOKEN":"runnable-environment-value-must-not-be-exported"}}
            ]}
        })
    };
    let a = "variable MODE = default_mode\nA { files a.sail, shared.sail }\n";
    let mut server = ServerFixture::open_with_options(
        "project-runnables",
        &[
            ("a.sail_project", a),
            ("b.sail_project", "variable MODE = default_mode\nB { files b.sail, shared.sail }\n"),
            ("a.sail", "val a : int\n"),
            ("b.sail", "val b : int\n"),
            ("shared.sail", "val shared : int\n"),
        ],
        Root::Real,
        None,
        settings("adapter-one"),
    );
    assert_eq!(
        server.capabilities["experimental"]["runnables"]["kinds"],
        json!(["sail-check", "shell"])
    );
    let list = |s: &mut ServerFixture| {
        s.ask("experimental/runnables", json!({"textDocument":{"uri":s.url("shared.sail")}}))
    };
    let first = list(&mut server);
    let entries = first.as_array().unwrap();
    assert_eq!(entries.len(), 5, "{first}");
    assert!(entries.iter().all(|r| r.get("disabled").is_none()), "{first}");
    let export = entries.iter().find(|r| r["args"]["kind"] == "export").unwrap();
    assert_eq!(export["args"]["program"], "/tool chain/chosen-sail");
    assert_eq!(
        export["args"]["args"],
        json!(["-c", "--project", "a.sail_project", "--variable", "MODE=A value", "A"])
    );
    let suites: Vec<_> = entries.iter().filter(|r| r["args"]["kind"] == "test").collect();
    let contexts: Vec<serde_json::Value> = suites
        .iter()
        .map(|r| serde_json::from_str(r["args"]["args"][0].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(contexts[0]["variables"]["MODE"], "A value");
    assert_eq!(contexts[0]["modules"], json!(["A"]));
    assert_eq!(contexts[1]["variables"]["MODE"], "B value");
    assert_eq!(contexts[1]["modules"], json!(["B"]));
    assert_ne!(contexts[0]["manifest"], contexts[1]["manifest"]);
    let report = server.ask("sail-lsp/diagnosticReport", json!({}));
    assert!(!report.to_string().contains("runnable-environment-value-must-not-be-exported"));
    assert_eq!(
        report["effectiveConfiguration"]["runnables"]["tasks"][1]["environment"]["API_TOKEN"],
        "<redacted>"
    );
    server.configure(settings("adapter-two"));
    let changed = list(&mut server);
    let updated = changed.as_array().unwrap().iter().find(|r| r["id"] == suites[0]["id"]).unwrap();
    assert_ne!(updated["fingerprint"], suites[0]["fingerprint"]);
    assert_eq!(updated["args"]["program"], "adapter-two");
    server.did_open("a.sail_project", &format!("{a}// unsaved manifest\n"));
    let draft = list(&mut server);
    let entries = draft.as_array().unwrap();
    assert!(
        entries
            .iter()
            .filter(|r| r["project"]["uri"] == server.url("a.sail_project").as_str())
            .all(|r| r["disabled"].as_str().unwrap().contains("manifest")),
        "{draft}"
    );
    assert!(
        entries
            .iter()
            .filter(|r| r["project"]["uri"] == server.url("b.sail_project").as_str())
            .all(|r| r.get("disabled").is_none()),
        "{draft}"
    );
}

#[test]
#[cfg(unix)]
fn project_runnable_check_runs_with_automatic_flycheck_disabled() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    let mut server = ServerFixture::new_project(
        "manual-runnable-check",
        &[("main.sail", "function answer() -> int = 42\n")],
    );
    let log = server.base.join("compiler-arguments.txt");
    let checker = server.base.join("chosen compiler");
    std::fs::write(
        &checker,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --project) printf '%s\\n' \"$PWD\" \"$@\" > '{}' ;;\nesac\n",
            log.display().to_string().replace('\'', "'\"'\"'")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o755)).unwrap();
    server.configure(json!({"flycheck":{"enable":false,"command":checker,"debounceMs":0},"workspace":{"projectModules":["Main"],"projectVariables":{"CHOSEN":"value with spaces"}}}));
    server.log.clear();
    server.wait_until("project configuration reload", |s| {
        s.log.iter().any(|line| line.contains("ready"))
    });
    assert!(!log.exists(), "disabled automatic checks must not launch a compiler");
    let list = server
        .ask("experimental/runnables", json!({"textDocument":{"uri":server.url("main.sail")}}));
    let check = &list[0];
    assert_eq!(check["kind"], "sail-check");
    server.notify("sail-lsp/runFlycheck", check["args"].clone());
    let expected =
        "--project\nsail.proj\n--just-check\n--variable\nCHOSEN=value with spaces\nMain\n";
    server.wait_until("explicit compiler check", |_| {
        std::fs::read_to_string(&log).is_ok_and(|captured| captured.contains(expected))
    });
    let captured = std::fs::read_to_string(log).unwrap();
    assert!(captured.contains(expected), "{captured}");
}

#[test]
#[ignore = "requires a real stock Sail compiler in SAIL_ORACLE_BIN; exports C using the discovered task"]
fn project_runnable_exports_with_the_real_compiler_and_selected_configuration() {
    use serde_json::json;
    let compiler = PathBuf::from(
        std::env::var_os("SAIL_ORACLE_BIN").expect("set SAIL_ORACLE_BIN to a stock Sail compiler"),
    );
    let environment: std::collections::BTreeMap<String, String> =
        std::env::var_os("SAIL_RUNNABLE_PLUGIN_DIR")
            .map(|path| {
                ("SAIL_PLUGIN_DIR".into(), PathBuf::from(path).to_string_lossy().into_owned())
            })
            .into_iter()
            .collect();
    let mut server = ServerFixture::open_with_options("runnable-real-export", &[
        ("sail.proj", "variable CHOOSE = false\nMain { files if $CHOOSE then chosen.sail else broken.sail }\n"),
        ("chosen.sail", "function answer() -> int = 42\nval main : unit -> unit\nfunction main() = ()\n"),
        ("broken.sail", "function this is deliberately invalid\n"),
    ], Root::Real, None, json!({
        "workspace":{"projectVariables":{"CHOOSE":false},"projectModules":["NotTheRoot"],"strictBitvector":true,
            "linkedProjects":[{"projectFile":"sail.proj","projectVariables":{"CHOOSE":true},"projectModules":["Main"]}]},
        "flycheck":{"command":compiler},
        "runnables":{"tasks":[{"label":"Export C","kind":"export","command":"${sail}","args":["-c","${projectArgs}","-o","${projectDir}/output name/model"],"environment":environment}]}
    }));
    let list = server
        .ask("experimental/runnables", json!({"textDocument":{"uri":server.url("chosen.sail")}}));
    let task: crate::lsp_ext::Runnable = serde_json::from_value(list[1].clone()).unwrap();
    assert!(task.disabled.is_none(), "{list}");
    let crate::lsp_ext::RunnableCommand::Process(args) = task.command else {
        panic!("export process expected")
    };
    assert_eq!(std::path::Path::new(&args.program), compiler);
    std::fs::create_dir_all(server.root.join("output name")).unwrap();
    let version = std::process::Command::new(&args.program)
        .arg("--version")
        .envs(&args.environment)
        .env_remove("SAIL_DIR")
        .output()
        .unwrap();
    assert!(version.status.success());
    let result = std::process::Command::new(&args.program)
        .args(&args.args)
        .current_dir(&args.cwd)
        .envs(&args.environment)
        .env_remove("SAIL_DIR")
        .output()
        .expect("compiler process must start");
    if let Some(directory) = std::env::var_os("SAIL_RUNNABLE_AUDIT_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("invocation.json"), serde_json::to_string_pretty(&json!({
            "program":args.program,"args":args.args,"cwd":args.cwd,"environment":args.environment,
            "version":String::from_utf8_lossy(&version.stdout),"exitCode":result.status.code(),
            "stdout":String::from_utf8_lossy(&result.stdout),"stderr":String::from_utf8_lossy(&result.stderr),
        })).unwrap()).unwrap();
        if result.status.success() {
            std::fs::copy(server.root.join("output name/model.c"), directory.join("model.c"))
                .unwrap();
        }
    }
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let generated = std::fs::read_to_string(server.root.join("output name/model.c")).unwrap();
    assert!(generated.contains("zmain"), "selected program entry must reach the generated model");
}

/// How long a scenario waits for the server to answer before failing.
const PATIENCE: Duration = Duration::from_secs(60);

/// Whether the client reaches the workspace directly or through a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    Real,
    #[cfg(unix)]
    Symlinked,
    Detached,
    EmptyFolders,
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
    messages: Vec<lsp_types::ShowMessageParams>,
    statuses: Vec<crate::lsp_ext::ServerStatusParams>,
    graph_changes: usize,
    /// The method of every request the server sent the client.
    server_requests: Vec<String>,
    shown_documents: Vec<serde_json::Value>,
    load_failed: bool,
    capabilities: serde_json::Value,
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// What a client says it can take, beyond what every fixture declares.
#[derive(Clone, Debug)]
struct ClientProfile {
    /// `completion.completionItem.snippetSupport`: `${1:x}` expands.
    completion_snippets: bool,
    /// Pulls diagnostics, as VS Code does, and accepts
    /// `workspace/diagnostic/refresh`.
    pull_diagnostics: bool,
    /// Registers no file watcher, so the server watches the folder itself.
    native_watch: bool,
    /// `inlayHint.resolveSupport` for the properties VS Code resolves lazily.
    inlay_hint_resolve: bool,
    /// The `experimental` section, where rust-analyzer's protocol extensions
    /// are negotiated. The VS Code client's set by default, less
    /// `snippetTextEdit`, which the scenarios about snippets declare; a
    /// scenario about a client without one says so here.
    experimental: serde_json::Value,
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self {
            completion_snippets: false,
            pull_diagnostics: false,
            native_watch: false,
            inlay_hint_resolve: false,
            experimental: serde_json::json!({
                "serverStatusNotification": true,
                "commands": { "commands": ["sail-lsp.showReferences"] },
            }),
        }
    }
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
        Self::open_with_options(name, files, kind, imports, serde_json::Value::Null)
    }

    fn open_with_options(
        name: &str,
        files: &[(&str, &str)],
        kind: Root,
        imports: Option<bool>,
        initialization_options: serde_json::Value,
    ) -> ServerFixture {
        Self::open_with_client(
            name,
            files,
            kind,
            imports,
            initialization_options,
            Default::default(),
        )
    }

    /// A workspace whose client pulls diagnostics, as VS Code's does, and
    /// accepts `workspace/diagnostic/refresh`.
    fn new_pulling(name: &str, files: &[(&str, &str)]) -> ServerFixture {
        Self::open_with_client(
            name,
            files,
            Root::Real,
            None,
            serde_json::Value::Null,
            ClientProfile { pull_diagnostics: true, ..Default::default() },
        )
    }

    /// A workspace whose client registers no file watcher, so the server
    /// watches the folder itself — the path a client without
    /// `didChangeWatchedFiles.dynamicRegistration` takes.
    fn new_native_watching(name: &str, files: &[(&str, &str)]) -> ServerFixture {
        Self::open_with_client(
            name,
            files,
            Root::Real,
            None,
            serde_json::Value::Null,
            ClientProfile { native_watch: true, ..Default::default() },
        )
    }

    fn open_with_client(
        name: &str,
        files: &[(&str, &str)],
        kind: Root,
        imports: Option<bool>,
        initialization_options: serde_json::Value,
        profile: ClientProfile,
    ) -> ServerFixture {
        let guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

        let base = std::env::temp_dir()
            .join(format!("sail_lsp_server_check_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // `Root::Symlinked` puts the files in `<base>/real` and hands the
        // client `<base>/link`, so every path it sends crosses the link.
        let (root, contents_at) = match kind {
            Root::Real | Root::Detached | Root::EmptyFolders => (base.clone(), base.clone()),
            #[cfg(unix)]
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
            messages: Vec::new(),
            statuses: Vec::new(),
            graph_changes: 0,
            server_requests: Vec::new(),
            shown_documents: Vec::new(),
            load_failed: false,
            capabilities: serde_json::Value::Null,
            _guard: guard,
        };
        fixture.initialize(imports, kind, initialization_options, profile);
        fixture
    }

    fn initialize(
        &mut self,
        imports: Option<bool>,
        kind: Root,
        initialization_options: serde_json::Value,
        profile: ClientProfile,
    ) {
        let ClientProfile {
            completion_snippets: snippets,
            pull_diagnostics: pull,
            native_watch,
            inlay_hint_resolve,
            experimental,
        } = profile;
        let root_url = Url::from_directory_path(&self.root).expect("root url");
        let has_workspace = kind != Root::Detached;
        let folders = match kind {
            Root::Detached => serde_json::Value::Null,
            Root::EmptyFolders => serde_json::json!([]),
            _ => serde_json::json!([{ "uri": root_url, "name": "fixture" }]),
        };
        let completion_properties = if imports.unwrap_or(false) {
            vec!["documentation", "additionalTextEdits"]
        } else {
            vec!["documentation"]
        };
        let mut text_document = if imports.is_some() {
            serde_json::json!({
                "completion": { "completionItem": { "resolveSupport": { "properties": completion_properties } } },
                "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["quickfix"] } } }
            })
        } else {
            serde_json::json!({})
        };
        if snippets {
            text_document["completion"]["completionItem"]["snippetSupport"] =
                serde_json::json!(true);
        }
        if inlay_hint_resolve {
            text_document["inlayHint"] = serde_json::json!({ "resolveSupport": { "properties": [
                "tooltip", "textEdits", "label.tooltip", "label.location", "label.command",
            ] } });
        }
        let mut workspace = serde_json::json!({
            "workspaceFolders": true,
            "workspaceEdit": { "documentChanges": true, "resourceOperations": ["create"] },
            // Dynamic registration, so the server does not start
            // its own watcher: a scenario that wants the watcher
            // path says so by not asking for this.
            "didChangeWatchedFiles": { "dynamicRegistration": !native_watch },
        });
        if pull {
            text_document["diagnostic"] = serde_json::json!({ "dynamicRegistration": false });
            workspace["diagnostics"] = serde_json::json!({ "refreshSupport": true });
        }
        let id = self.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": has_workspace.then_some(&root_url),
                "workspaceFolders": folders,
                "initializationOptions": initialization_options,
                "capabilities": {
                    "window": { "workDoneProgress": true, "showDocument": {"support":true} },
                    "textDocument": text_document,
                    "workspace": workspace,
                    "experimental": experimental,
                },
            }),
        );
        let response = self.wait_for_response(id);
        let result = response.result.expect("initialize response");
        assert_eq!(result["serverInfo"]["version"], crate::version::version());
        self.capabilities = result["capabilities"].clone();
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
                        self.load_failed |= params.typ == lsp_types::MessageType::ERROR
                            && params.message.starts_with("Failed to load Sail project");
                        self.log.push(params.message);
                    }
                    "window/showMessage" => {
                        self.messages.push(serde_json::from_value(n.params).expect("showMessage"));
                    }
                    "sail-lsp/projectGraphChanged" => {
                        self.graph_changes += 1;
                    }
                    "sail-lsp/serverStatus" => {
                        self.statuses.push(serde_json::from_value(n.params).expect("serverStatus"));
                    }
                    _ => {}
                },
                // A server request with no answer stalls whatever asked it.
                Message::Request(r) => {
                    self.server_requests.push(r.method.clone());
                    if r.method == "window/showDocument" {
                        self.shown_documents.push(r.params.clone());
                    }
                    let result = if r.method == "window/showDocument" {
                        serde_json::json!({"success":true})
                    } else {
                        serde_json::Value::Null
                    };
                    let response = Response { id: r.id, result: Some(result), error: None };
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
        self.fake_checker_for(None, Duration::ZERO, prints)
    }

    /// The same stand-in, answering only when asked about `file` (every run,
    /// for `None`) and taking `delay` to do so — a real `sail` run takes
    /// seconds, and what the server does meanwhile is part of what it owns.
    /// Every invocation's arguments are recorded for [`Self::fake_checker_runs`].
    #[cfg(unix)]
    pub(crate) fn fake_checker_for(
        &self,
        file: Option<&str>,
        delay: Duration,
        prints: &str,
    ) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.base.join("fake-sail");
        let runs = self.base.join("fake-sail-runs");
        let only = file.map_or("*".to_owned(), |file| format!("*{file}*"));
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n{only}) sleep {}\ncat >&2 <<'UPSTREAM'\n{prints}\nUPSTREAM\nexit 1;;\nesac\nexit 0\n",
            runs.display(),
            delay.as_secs_f64()
        );
        std::fs::write(&path, script).expect("fake checker");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make it runnable");
        path
    }

    /// The arguments of every run of the fake checker so far, one per run.
    #[cfg(unix)]
    pub(crate) fn fake_checker_runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("fake-sail-runs"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Pump until no workspace scan is pending or running — the one
    /// synchronisation point a client has, since settings changes can scan.
    fn wait_until_scans_settle(&mut self) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            let status = self.ask("sail-lsp/analyzerStatus", serde_json::Value::Null);
            if status.as_str().is_some_and(|s| s.contains("reload pending:   false")) {
                return;
            }
            self.pump(Duration::from_millis(50));
        }
        panic!("waited {PATIENCE:?} for scans to settle; log so far: {:?}", self.log);
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
/// `eviction_waits_for_the_pool_and_the_next_wave_publishes_its_findings`
/// also holds a worker snapshot across an attempted idle collection.
#[test]
fn initialization_options_select_the_project_before_discovery() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::open_with_options(
        "initialization-options-project",
        &[
            ("model/selected.sail_project", "Main { files types.sail }"),
            ("model/other.sail_project", "Main { files other.sail }"),
            ("model/types.sail", source),
            ("model/other.sail", "function other() -> int = 0\n"),
        ],
        Root::Real,
        None,
        serde_json::json!({"workspace": {"projectFile": "model/selected.sail_project"}}),
    );
    assert!(!server.load_failed, "{:?}", server.log);
    server.did_open("model/types.sail", source);
    let diagnostics = server.pull("model/types.sail");
    assert_eq!(errors(&diagnostics), 1, "{diagnostics:?}");
    assert!(!mentions(&diagnostics, "not part of a loaded Sail project"), "{diagnostics:?}");
    assert!(server
        .log
        .iter()
        .any(|m| m.contains("Loaded Sail project:") && m.contains("selected.sail_project")));
}

#[test]
fn virtual_sources_support_navigation_unsaved_edits_and_provider_changes() {
    let generated = "function generated(x : int) -> int = x\n";
    let incompatible = "function generated(x : bool) -> bool = x\n";
    let caller = "function caller() -> int = generated(1)\n";
    let mut server = ServerFixture::open_with_options(
        "virtual-sources",
        &[
            ("sail.proj", "Main { files __virtual defs.sail, main.sail }"),
            ("build/generated.sail", generated),
            ("build/other.sail", incompatible),
            ("main.sail", caller),
        ],
        Root::Real,
        None,
        serde_json::json!({"workspace": {"generatedFiles": {"defs.sail": "build/generated.sail"}}}),
    );
    assert!(!server.load_failed, "{:?}", server.log);
    server.did_open("main.sail", caller);
    server.did_open("build/generated.sail", generated);
    server.wait_for_diagnostics("main.sail", "generated declaration is available", |d| {
        errors(d) == 0
    });
    let position = serde_json::json!({
        "textDocument": {"uri": server.url("main.sail")},
        "position": {"line": 0, "character": caller.find("generated").unwrap()},
    });
    let navigation = server.ask("textDocument/definition", position.clone());
    assert_eq!(navigation[0]["uri"], server.url("build/generated.sail").as_str());
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("build/generated.sail"), "version": 2},
            "contentChanges": [{"text": incompatible}],
        }),
    );
    server.wait_for_diagnostics("main.sail", "generated overlay invalidates the caller", |d| {
        errors(d) > 0
    });
    server.did_close("build/generated.sail");
    server.wait_for_diagnostics(
        "main.sail",
        "closing generated overlay restores producer output",
        |d| errors(d) == 0,
    );
    for (text, should_fail) in [(incompatible, true), (generated, false)] {
        std::fs::write(server.path("build/generated.sail"), text).unwrap();
        server.notify(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({
                "changes": [{"uri": server.url("build/generated.sail"), "type": 2}],
            }),
        );
        server.wait_for_diagnostics(
            "main.sail",
            "producer output change reaches the caller",
            |d| (errors(d) > 0) == should_fail,
        );
    }
    server.configure(
        serde_json::json!({"workspace": {"generatedFiles": {"defs.sail": "build/other.sail"}}}),
    );
    server.wait_for_diagnostics("main.sail", "changing provider updates dependencies", |d| {
        errors(d) > 0
    });
    let navigation = server.ask("textDocument/definition", position);
    assert_eq!(navigation[0]["uri"], server.url("build/other.sail").as_str());
}

#[test]
fn empty_workspace_folders_use_the_clients_root_uri() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::open(
        "empty-workspace-folders",
        &[("model/riscv.sail_project", "Main { files types.sail }"), ("model/types.sail", source)],
        Root::EmptyFolders,
    );
    server.did_open("model/types.sail", source);
    let diagnostics = server.pull("model/types.sail");
    assert_eq!(errors(&diagnostics), 1, "{diagnostics:?}");
    assert!(!mentions(&diagnostics, "not part of a loaded Sail project"), "{diagnostics:?}");
    assert!(server
        .log
        .iter()
        .any(|m| m.contains("Loaded Sail project:") && m.contains("riscv.sail_project")));
}

#[test]
fn advertised_workspace_folder_changes_attach_already_open_files() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::open(
        "late-workspace-folder",
        &[("model/riscv.sail_project", "Main { files types.sail }"), ("model/types.sail", source)],
        Root::Detached,
    );
    server.did_open("model/types.sail", source);
    assert!(mentions(&server.pull("model/types.sail"), "not part of a loaded Sail project"));
    // Zed only sends additions when the server advertises changeNotifications.
    let folders = &server.capabilities["workspace"]["workspaceFolders"];
    assert_eq!(folders["supported"], true, "{folders}");
    assert_eq!(folders["changeNotifications"], true, "{folders}");
    let root = Url::from_directory_path(&server.root).unwrap();
    server.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [{"uri": root, "name": "fixture"}], "removed": []}
        }),
    );
    server.wait_for_diagnostics(
        "model/types.sail",
        "project attached after initialization",
        |ds| errors(ds) == 1 && !mentions(ds, "not part of a loaded Sail project"),
    );
    server.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [], "removed": [{"uri": root, "name": "fixture"}]}
        }),
    );
    server.wait_for_diagnostics(
        "model/types.sail",
        "project detached after removing its folder",
        |ds| errors(ds) == 0 && mentions(ds, "not part of a loaded Sail project"),
    );
}

#[test]
fn missing_project_sources_load_remaining_files_and_recover_when_created() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::new(
        "missing-project-sources",
        &[
            ("sail.proj", "Main { files absent.sail, other.sail, main.sail }"),
            ("main.sail", source),
        ],
    );
    assert!(!server.load_failed, "{:?}", server.log);
    server.did_open("main.sail", source);
    let diagnostics = server.pull("main.sail");
    assert_eq!(errors(&diagnostics), 1, "{diagnostics:?}");
    assert!(!mentions(&diagnostics, "not part of a loaded Sail project"), "{diagnostics:?}");
    assert_eq!(server.messages.len(), 1, "{:?}", server.messages);
    let message = &server.messages[0].message;
    for expected in ["sail.proj", "absent.sail", "other.sail", "incomplete"] {
        assert!(message.contains(expected), "{message}");
    }

    server.create_on_disk("absent.sail", "function present() -> int = 0\n");
    server.create_on_disk("other.sail", "function other() -> int = 1\n");
    server.wait_until("project loading to recover", |f| {
        f.log.iter().any(|m| m.contains("Sail project loading recovered"))
    });
    server.did_open("absent.sail", "function present() -> int = 0\n");
    assert!(server.pull("absent.sail").is_empty());
    assert_eq!(errors(&server.pull("main.sail")), 1);
}

#[test]
fn invalid_manifest_reports_its_cause_to_the_editor_and_unlinked_files() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::new(
        "invalid-manifest-notification",
        &[("sail.proj", "Main { requires Unknown }"), ("main.sail", source)],
    );
    server.did_open("main.sail", source);
    let diagnostics = server.pull("main.sail");
    assert!(mentions(&diagnostics, "Unknown"), "{diagnostics:?}");
    assert!(mentions(&diagnostics, "sail.proj"), "{diagnostics:?}");
    server.wait_for_diagnostics("main.sail", "push diagnostics to carry the cause", |ds| {
        mentions(ds, "Unknown") && mentions(ds, "sail.proj")
    });
    assert_eq!(server.messages.len(), 1, "{:?}", server.messages);
    assert!(server.messages[0].message.contains("Unknown"));
    assert!(server.messages[0].message.contains("No project model is available"));

    let report = server.ask(
        "textDocument/diagnostic",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")}
        }),
    );
    server.create_on_disk("sail.proj", "Main { requires AnotherUnknown }");
    server.wait_for_diagnostics("main.sail", "changed loading cause to be republished", |ds| {
        mentions(ds, "AnotherUnknown")
    });
    let changed = server.ask(
        "textDocument/diagnostic",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "previousResultId": report["resultId"],
        }),
    );
    assert_eq!(changed["kind"], "full", "{changed}");
    assert_ne!(changed["resultId"], report["resultId"]);

    // Recovery must also clear the old cause when the file stays unlinked.
    server.create_on_disk("sail.proj", "Main { files [] }");
    server.wait_for_diagnostics("main.sail", "loading cause to clear", |ds| {
        mentions(ds, "not part of a loaded Sail project") && !mentions(ds, "Failed to load")
    });
    server.create_on_disk("sail.proj", "Main { files main.sail }");
    server.wait_for_diagnostics("main.sail", "fixed manifest to restore analysis", |ds| {
        errors(ds) == 1
            && !mentions(ds, "Unknown")
            && !mentions(ds, "not part of a loaded Sail project")
    });
    assert!(server.log.iter().any(|m| m.contains("Sail project loading recovered")));
}

/// From a file to what brings it into the project: the manifest module
/// that lists it, or the `$include` that names it, and the manifest entry
/// behind either.
#[test]
fn a_file_leads_to_its_module_and_manifest() {
    let manifest = "Lib {\n  files lib.sail\n}\nMain {\n  requires Lib\n  files main.sail\n}\n";
    let main = "$include \"inc.sail\"\n";
    let mut server = ServerFixture::new(
        "project-navigation",
        &[
            ("sail.proj", manifest),
            ("lib.sail", "val f : unit -> unit\n"),
            ("main.sail", main),
            ("inc.sail", "val g : unit -> unit\n"),
            ("stray.sail", ""),
        ],
    );
    server.did_open("main.sail", main);
    server.pull("main.sail");
    let doc = |server: &ServerFixture, rel: &str| serde_json::json!({ "textDocument": { "uri": server.url(rel) } });
    let at = |v: &serde_json::Value| {
        (
            v["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned(),
            v["range"]["start"]["line"].as_u64().unwrap(),
            v["range"]["start"]["character"].as_u64().unwrap(),
            v["range"]["end"]["character"].as_u64().unwrap(),
        )
    };
    let entry = ("sail.proj".to_owned(), 5, 8, 17);

    let parents = server.ask("sail-lsp/parentModule", doc(&server, "main.sail"));
    let parents: Vec<_> = parents.as_array().unwrap().iter().map(at).collect();
    assert_eq!(parents, [("sail.proj".to_owned(), 3, 0, 4)]);
    let manifest_entry = server.ask("sail-lsp/openProjectManifest", doc(&server, "main.sail"));
    assert_eq!(at(&manifest_entry), entry);

    let parents = server.ask("sail-lsp/parentModule", doc(&server, "inc.sail"));
    let parents: Vec<_> = parents.as_array().unwrap().iter().map(at).collect();
    assert_eq!(parents, [("main.sail".to_owned(), 0, 10, 18)]);
    let manifest_entry = server.ask("sail-lsp/openProjectManifest", doc(&server, "inc.sail"));
    assert_eq!(at(&manifest_entry), entry, "an included file opens its includer's entry");

    assert!(server.ask("sail-lsp/openProjectManifest", doc(&server, "stray.sail")).is_null());
    assert_eq!(
        server.ask("sail-lsp/parentModule", doc(&server, "stray.sail")),
        serde_json::json!([])
    );
}

/// Inside a manifest: a module reference goes to its header, a path to its
/// file, completion offers modules after `requires`; the manifest is never
/// analysed as Sail. A file no module lists gets a fix that lists it, and
/// the project then loads it.
#[test]
fn a_manifest_is_edited_as_a_manifest() {
    let manifest =
        "Lib {\n  files core/lib.sail\n}\nMain {\n  requires Lib\n  files core/main.sail\n}\n";
    let mut server = ServerFixture::new(
        "manifest-editing",
        &[
            ("sail.proj", manifest),
            ("core/lib.sail", "val f : unit -> unit\n"),
            ("core/main.sail", "val g : unit -> unit\n"),
            ("core/new.sail", "val h : unit -> int\nfunction h() = true\n"),
        ],
    );
    server.did_open("sail.proj", manifest);
    let doc = server.url("sail.proj");
    let at = |line: u32, character: u32| serde_json::json!({ "textDocument": { "uri": doc }, "position": { "line": line, "character": character } });
    let target = server.ask("textDocument/definition", at(4, 12));
    assert_eq!(target[0]["uri"], serde_json::json!(doc));
    assert_eq!(target[0]["range"]["start"], serde_json::json!({ "line": 0, "character": 0 }));
    let target = server.ask("textDocument/definition", at(1, 14));
    assert_eq!(target[0]["uri"], serde_json::json!(server.url("core/lib.sail")));
    let target =
        server.ask("textDocument/definition", at(1, manifest.lines().nth(1).unwrap().len() as u32));
    assert_eq!(
        target[0]["uri"],
        serde_json::json!(server.url("core/lib.sail")),
        "cursor at the path's end"
    );

    let edited = manifest.replace("requires Lib", "requires Lib, ");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({ "textDocument": { "uri": doc, "version": 2 }, "contentChanges": [{ "text": edited }] }),
    );
    let items = server.ask("textDocument/completion", at(4, 16));
    let labels: Vec<_> =
        items.as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Lib"], "Main does not require itself");
    let hover = server.ask("textDocument/hover", at(4, 12));
    assert!(
        hover["contents"]["value"].as_str().unwrap().contains("project module `Lib`"),
        "{hover}"
    );
    let references = server.ask(
        "textDocument/references",
        serde_json::json!({
            "textDocument": {"uri": doc}, "position": {"line": 4, "character": 12},
            "context": {"includeDeclaration": true}
        }),
    );
    assert_eq!(references.as_array().unwrap().len(), 2, "{references}");
    let prepared = server.ask("textDocument/prepareRename", at(4, 12));
    assert_eq!(prepared["placeholder"], "Lib");
    let renamed = server.ask(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": doc}, "position": {"line": 4, "character": 12},
            "newName": "Library"
        }),
    );
    assert_eq!(renamed["changes"][doc.as_str()].as_array().unwrap().len(), 2, "{renamed}");
    let symbols = server
        .ask("textDocument/documentSymbol", serde_json::json!({"textDocument": {"uri": doc}}));
    let symbol_names: Vec<_> =
        symbols.as_array().unwrap().iter().map(|symbol| symbol["name"].as_str().unwrap()).collect();
    assert_eq!(symbol_names, ["Lib", "Main"]);
    server.notify(
        "textDocument/didChange",
        serde_json::json!({ "textDocument": { "uri": doc, "version": 3 }, "contentChanges": [{ "text": manifest }] }),
    );

    let paths = server.ask("textDocument/completion", at(5, 13));
    let path_labels: Vec<_> =
        paths.as_array().unwrap().iter().map(|item| item["label"].as_str().unwrap()).collect();
    assert_eq!(path_labels, ["core/lib.sail", "core/main.sail", "core/new.sail"]);
    let formatted = server.ask(
        "textDocument/formatting",
        serde_json::json!({"textDocument": {"uri": doc}, "options": {"tabSize": 2, "insertSpaces": true}}),
    );
    assert_eq!(formatted, serde_json::json!([]), "an already formatted manifest is stable");

    let new = "val h : unit -> int\nfunction h() = true\n";
    server.did_open("core/new.sail", new);
    let unlinked: Vec<_> = server
        .pull("core/new.sail")
        .into_iter()
        .filter(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
        .collect();
    assert_eq!(unlinked.len(), 1);
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": { "uri": server.url("core/new.sail") },
            "range": unlinked[0].range,
            "context": { "diagnostics": unlinked },
        }),
    );
    let fixes: Vec<_> = actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["title"].as_str().unwrap().starts_with("Add `"))
        .collect();
    let titles: Vec<_> = fixes.iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert_eq!(
        titles,
        [
            "Add `core/new.sail` to module `Lib` in sail.proj",
            "Add `core/new.sail` to module `Main` in sail.proj",
        ],
        "both modules list files beside it, so the user chooses"
    );
    assert_eq!(fixes[1]["edit"]["documentChanges"][0]["textDocument"]["version"], 3);
    let edit = &fixes[1]["edit"]["documentChanges"][0]["edits"][0];
    assert_eq!(edit["newText"], ", core/new.sail");
    assert_eq!(edit["range"]["start"], serde_json::json!({ "line": 5, "character": 22 }));

    // While the preview is open the user inserts a line. The old action's
    // precondition remains version 3; a fresh action must describe version 4
    // and its shifted range. A delayed version-3 notification must not roll
    // the document back and make that stale preview appear applicable again.
    let shifted = format!("\n{manifest}");
    for (version, text) in [(4, shifted.as_str()), (3, manifest)] {
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": doc, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }
    let refreshed = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("core/new.sail")},
            "range": unlinked[0].range,
            "context": {"diagnostics": unlinked},
        }),
    );
    let refreshed = refreshed.as_array().unwrap().iter().find(|a| a["title"] == titles[1]).unwrap();
    let document = &refreshed["edit"]["documentChanges"][0];
    assert_eq!(document["textDocument"]["version"], 4);
    assert_ne!(
        document["textDocument"]["version"],
        fixes[1]["edit"]["documentChanges"][0]["textDocument"]["version"]
    );
    assert_eq!(
        document["edits"][0]["range"]["start"],
        serde_json::json!({"line": 6, "character": 22})
    );

    server.create_on_disk(
        "sail.proj",
        &manifest.replace("core/main.sail", "core/main.sail, core/new.sail"),
    );
    server.wait_for_diagnostics("core/new.sail", "the added file to be checked", |ds| {
        errors(ds) == 1 && !mentions(ds, "not part of a loaded Sail project")
    });
}

#[test]
fn manifest_paths_use_expressions_and_fixes_only_offer_active_modules() {
    let manifest = "variable BASE = \"sub\"\nA { files a.sail }\nB { files $BASE / \"b.sail\" }\n";
    let source = "function f() -> int = 0\n";
    let mut server = ServerFixture::open_with_options(
        "manifest-active-fix",
        &[
            ("sail.proj", manifest),
            ("a.sail", source),
            ("sub/b.sail", source),
            ("sub/new.sail", "function bad() -> int = true\n"),
        ],
        Root::Real,
        None,
        serde_json::json!({"workspace": {"projectModules": ["A"]}}),
    );
    server.did_open("sail.proj", manifest);
    let doc = server.url("sail.proj");
    let definition = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": doc}, "position": {"line": 2, "character": 22}
        }),
    );
    assert_eq!(definition[0]["uri"], serde_json::json!(server.url("sub/b.sail")));
    server.did_open("sub/new.sail", "function bad() -> int = true\n");
    let unlinked: Vec<_> = server
        .pull("sub/new.sail")
        .into_iter()
        .filter(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
        .collect();
    assert_eq!(unlinked.len(), 1);
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("sub/new.sail")},
            "range": unlinked[0].range, "context": {"diagnostics": unlinked}
        }),
    );
    let fixes: Vec<_> = actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["title"].as_str().unwrap().starts_with("Add `"))
        .collect();
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0]["title"], "Add `sub/new.sail` to module `A` in sail.proj");
    let edit: lsp_types::TextEdit =
        serde_json::from_value(fixes[0]["edit"]["documentChanges"][0]["edits"][0].clone()).unwrap();
    let index = ide_db::line_index::LineIndex::new(manifest);
    let start = crate::from_proto::offset(&index, edit.range.start);
    let end = crate::from_proto::offset(&index, edit.range.end);
    let mut updated = manifest.to_owned();
    updated.replace_range(start..end, &edit.new_text);
    server.create_on_disk("sail.proj", &updated);
    server.wait_for_diagnostics("sub/new.sail", "active module fix loads its file", |ds| {
        errors(ds) == 1 && !mentions(ds, "not part of a loaded Sail project")
    });
}

/// A broken manifest is underlined where it is broken, in the manifest:
/// the unknown module in `requires`, the `files` entry naming a missing
/// file. Fixing it clears the manifest.
#[test]
fn manifest_problems_are_published_on_the_manifest() {
    let source = "function f() -> int = 1\n";
    let manifest = "Main {\n  requires Nope\n  files main.sail\n}\n";
    let mut server = ServerFixture::new(
        "manifest-diagnostics",
        &[("sail.proj", manifest), ("main.sail", source)],
    );
    server.wait_for_diagnostics("sail.proj", "unknown module underlined", |ds| {
        ds.len() == 1 && ds[0].message.contains("Nope")
    });
    let d = server.published[&server.url("sail.proj")][0].clone();
    assert_eq!((d.range.start.line, d.range.start.character, d.range.end.character), (1, 11, 15));
    assert_eq!(d.code, Some(lsp_types::NumberOrString::String("unknown-module".into())));
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("sail.proj")}, "range": d.range,
            "context": {"diagnostics": [d]}
        }),
    );
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["title"] == "Declare project module `Nope`"),
        "{actions}"
    );

    server.create_on_disk(
        "sail.proj",
        "Main {\n  files if $MISSING then main.sail else main.sail\n}\n",
    );
    server.wait_for_diagnostics("sail.proj", "unknown variable underlined", |ds| {
        ds.len() == 1 && ds[0].message.contains("MISSING")
    });
    let d = server.published[&server.url("sail.proj")][0].clone();
    assert_eq!(d.code, Some(lsp_types::NumberOrString::String("unknown-variable".into())));
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("sail.proj")}, "range": d.range,
            "context": {"diagnostics": [d]}
        }),
    );
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["title"] == "Declare project variable `MISSING`"),
        "{actions}"
    );

    server.create_on_disk("sail.proj", "Main {\n  files main.sail, gone.sail\n}\n");
    server.wait_for_diagnostics("sail.proj", "missing file underlined", |ds| {
        ds.len() == 1 && ds[0].message.contains("gone.sail")
    });
    let d = server.published[&server.url("sail.proj")][0].clone();
    assert_eq!((d.range.start.line, d.range.start.character, d.range.end.character), (1, 19, 28));
    assert_eq!(d.code, Some(lsp_types::NumberOrString::String("missing-file".into())));
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("sail.proj")}, "range": d.range,
            "context": {"diagnostics": [d]}
        }),
    );
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["title"] == "Create missing project source `gone.sail`"),
        "{actions}"
    );

    server.create_on_disk("sail.proj", "Main {\n  files main.sail\n}\n");
    server.wait_for_diagnostics("sail.proj", "fixed manifest cleared", |ds| ds.is_empty());
}

#[test]
fn invalid_manifest_reload_keeps_the_project_and_deduplicates_notifications() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::new(
        "invalid-manifest-reload",
        &[("sail.proj", "Main { files main.sail }"), ("main.sail", source)],
    );
    server.did_open("main.sail", source);
    assert_eq!(errors(&server.pull("main.sail")), 1);
    server.create_on_disk("sail.proj", "Main {");
    server.wait_until("failed reload notification", |f| !f.messages.is_empty());
    assert!(server.messages[0].message.contains("sail.proj"));
    assert!(server.messages[0].message.contains("Keeping the previous project model"));
    let diagnostics = server.pull("main.sail");
    assert_eq!(errors(&diagnostics), 1, "{diagnostics:?}");
    assert!(!mentions(&diagnostics, "not part of a loaded Sail project"));

    server.wait_until("first failed reload to finish", |f| {
        f.statuses.last().is_some_and(|s| {
            s.quiescent && s.message.as_deref().is_some_and(|m| m.contains("Failed to load"))
        })
    });
    server.statuses.clear();
    server.create_on_disk("sail.proj", "Main {");
    server.wait_until("duplicate failed reload to finish", |f| {
        f.statuses.iter().any(|s| !s.quiescent) && f.statuses.last().is_some_and(|s| s.quiescent)
    });
    assert_eq!(server.messages.len(), 1, "{:?}", server.messages);
    server.create_on_disk("sail.proj", "Main { files main.sail }");
    server.wait_until("reload recovery", |f| {
        f.log.iter().any(|m| m.contains("Sail project loading recovered"))
    });
    assert_eq!(server.messages.len(), 1, "{:?}", server.messages);
}

#[test]
fn failed_initial_load_stays_unlinked_until_explicit_configuration_recovers() {
    let source = "function bad() -> int = true\n";
    let mut server = ServerFixture::new(
        "failed-initial-project",
        &[
            ("sail.proj", "Broken { requires Unknown }"),
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
                "position": {"line": 0, "character": source.find(name).unwrap() + 2}
            }),
        );
        let rendered = hover["contents"]["value"].as_str().expect("hover text");
        assert_eq!(rendered.lines().next(), Some(expected), "{hover}");
        let start = source.find(name).unwrap();
        assert_eq!(
            hover["range"],
            serde_json::json!({
                "start": {"line": 0, "character": start},
                "end": {"line": 0, "character": start + name.len()},
            })
        );
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
            .as_chunks::<5>()
            .0
            .iter()
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
    let changes: serde_json::Map<String, serde_json::Value> = rename["documentChanges"]
        .as_array()
        .expect("versioned rename edits")
        .iter()
        .map(|doc| (doc["textDocument"]["uri"].as_str().unwrap().to_owned(), doc["edits"].clone()))
        .collect();
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
fn display_configuration_controls_lens_categories_hints_and_tokens() {
    let source = "val helper : int -> int\nfunction helper(value) = value\nfunction main() -> int = helper(1)\n";
    let mut server = ServerFixture::new_project("display-config", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let params = serde_json::json!({"textDocument": {"uri": server.url("main.sail")}});
    let all = server.ask("textDocument/codeLens", params.clone());
    for kind in ["refs", "impls"] {
        assert!(all.as_array().unwrap().iter().any(|lens| lens["data"]["kind"] == kind), "{all}");
        server.configure(serde_json::json!({"codeLens": {
            "references": kind == "refs", "implementations": kind == "impls",
        }}));
        let selected = server.ask("textDocument/codeLens", params.clone());
        let lenses = selected.as_array().unwrap();
        assert!(!lenses.is_empty());
        assert!(lenses.iter().all(|lens| lens["data"]["kind"] == kind), "{selected}");
    }
    let hints = serde_json::json!({"textDocument": {"uri": server.url("main.sail")},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 0}}});
    for enable in [false, true] {
        server.configure(serde_json::json!({"codeLens": {"enable": enable},
            "inlayHints": {"enable": enable}, "semanticTokens": {"enable": enable}}));
        for (method, params) in [
            ("textDocument/codeLens", params.clone()),
            ("textDocument/inlayHint", hints.clone()),
            ("textDocument/semanticTokens/full", params.clone()),
        ] {
            let response = server.ask(method, params);
            if enable {
                assert!(!response.is_null(), "{method}");
            } else {
                assert!(
                    response.is_null() || response.as_array().is_some_and(|items| items.is_empty()),
                    "{method}: {response}"
                );
            }
        }
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
    // Clicking shows what the title counted: it sent an empty list.
    assert_eq!(resolved["command"]["command"], "sail-lsp.showReferences", "{resolved}");
    let shown: Vec<String> = resolved["command"]["arguments"][2]
        .as_array()
        .expect("locations")
        .iter()
        .map(|l| l["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned())
        .collect();
    assert_eq!(shown, ["a.sail", "b.sail"], "{resolved}");
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
            "position": {"line": 0, "character": 5}, "newName": "renamed_repeated"
        }),
    );
    let response = server.wait_for_response(request);
    assert!(response.error.is_none(), "equivalent contexts can rename despite an existing duplicate-definition error: {response:?}");
    let edit = response.result.unwrap();
    let documents = edit["documentChanges"].as_array().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["textDocument"]["version"], 2);
    assert_eq!(documents[0]["edits"].as_array().unwrap().len(), 2, "{edit}");
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

#[test]
fn rapid_signature_edits_and_undo_recover_dependent_diagnostics() {
    let definition = "function identity(x : int) -> int = x\n";
    let incompatible = "function identity(x : bool) -> bool = x\n";
    let caller = "function caller(x : int) -> int = identity(x)\n";
    let mut server = ServerFixture::new_project(
        "rapid-signature-edits",
        &[("definition.sail", definition), ("caller.sail", caller)],
    );
    server.did_open("definition.sail", definition);
    server.did_open("caller.sail", caller);
    server.wait_for_diagnostics("caller.sail", "initial valid call", |d| errors(d) == 0);
    let change = |server: &mut ServerFixture, version, text| {
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": server.url("definition.sail"), "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    };
    change(&mut server, 2, incompatible);
    server.wait_for_diagnostics("caller.sail", "changed signature invalidates caller", |d| {
        errors(d) > 0
    });
    // A typing burst contains incomplete syntax and semantic changes. Undo
    // restores the exact original source, without saving either open file.
    for version in 3..33 {
        let text = if version % 2 == 0 { incompatible } else { "function identity(" };
        change(&mut server, version, text);
    }
    change(&mut server, 33, definition);
    server.wait_for_diagnostics("caller.sail", "undo clears dependent errors", |d| errors(d) == 0);
    server.wait_for_diagnostics("definition.sail", "undo clears syntax errors", |d| errors(d) == 0);
    assert_eq!(errors(&server.pull("caller.sail")), 0);
    assert_eq!(errors(&server.pull("definition.sail")), 0);
    server.pump(Duration::from_millis(300));
    assert_eq!(errors(&server.shown("caller.sail")), 0);
    assert_eq!(errors(&server.shown("definition.sail")), 0);
}

#[test]
fn shared_configuration_selects_the_project_and_saved_changes_reload_without_editor_settings() {
    let first =
        "[workspace]\nprojectFile = 'chosen.sail_project'\nprojectVariables = { FLAG = true }\n";
    let second = first.replace("true", "false");
    let source = "function chosen() -> int = 42\n";
    let mut server = ServerFixture::new(
        "shared-settings-project",
        &[
            (".sail-lsp.toml", first),
            ("sail.proj", "Wrong { files missing.sail }\n"),
            (
                "chosen.sail_project",
                "variable FLAG = false\nMain { files if $FLAG then a.sail else b.sail }\n",
            ),
            ("a.sail", source),
            ("b.sail", "function alternative() -> int = 0\n"),
        ],
    );
    server.did_open("a.sail", source);
    assert!(server
        .pull("a.sail")
        .iter()
        .all(|d| d.code != Some(lsp_types::NumberOrString::String("unlinked-file".into()))));
    let report = server.ask("sail-lsp/diagnosticReport", serde_json::json!({}));
    assert_eq!(
        report["folderConfigurations"][0]["configuration"]["workspace"]["projectVariables"]["FLAG"],
        true
    );
    server.create_on_disk(".sail-lsp.toml", &second);
    server.wait_for_diagnostics(
        "a.sail",
        "saved repository variable selects the other source",
        |ds| {
            ds.iter()
                .any(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
        },
    );
    server.create_on_disk(".sail-lsp.toml", "[workspace\n");
    server.wait_for_diagnostics(
        ".sail-lsp.toml",
        "invalid shared settings are reported on the settings file",
        |ds| !ds.is_empty(),
    );
    server.create_on_disk(".sail-lsp.toml", first);
    server.wait_for_diagnostics("a.sail", "repair selects the source again", |ds| {
        !ds.iter()
            .any(|d| d.code == Some(lsp_types::NumberOrString::String("unlinked-file".into())))
    });
    server.wait_for_diagnostics(".sail-lsp.toml", "repair clears the settings diagnostic", |ds| {
        ds.is_empty()
    });
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

/// A client that pulls is not also sent the same findings.
///
/// vscode-languageclient keeps pulled diagnostics in a collection of their
/// own, so a server that pushed as well showed every problem twice. It is
/// told to pull again instead.
#[test]
fn a_pulling_client_is_not_pushed_the_same_findings() {
    let mut server = ServerFixture::new_pulling(
        "pulling",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", A_FILE_WITH_ONE_ERROR)],
    );
    server.did_open("a.sail", A_FILE_WITH_ONE_ERROR);
    server.wait_until("a diagnostic refresh", |f| {
        f.server_requests.iter().any(|m| m == "workspace/diagnostic/refresh")
    });
    assert_eq!(errors(&server.pull("a.sail")), 1);
    server.pump(Duration::from_millis(500));
    assert!(server.shown("a.sail").is_empty(), "pushed as well: {:?}", server.shown("a.sail"));
}

#[test]
fn syntax_errors_are_not_duplicated_by_the_full_diagnostic_wave() {
    let source = "function f(x : int) -> int = {\n";
    let mut server = ServerFixture::new_project("syntax-push-pull", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let mut expected = server.pull("a.sail");
    expected.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    assert!(!expected.is_empty(), "fixture must have a syntax error");
    server
        .wait_for_diagnostics("a.sail", "push matches full diagnostics", |diags| diags == expected);
    server.pump(Duration::from_millis(300));
    assert_eq!(server.shown("a.sail"), expected);
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
fn flycheck_managed_cache_and_success_history_follow_saved_inputs() {
    use std::os::unix::fs::PermissionsExt;
    let source = "function identity(value : int) -> int = value\n";
    let mut server = ServerFixture::new_project("flycheck-cache-history", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let checker = server.base.join("cache-checker");
    std::fs::write(
        &checker,
        r#"#!/bin/sh
check=no
memo=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --just-check) check=yes ;;
    --memo-z3-path) shift; memo=$1 ;;
  esac
  shift
done
[ "$check" = yes ] || exit 1
[ -n "$memo" ] || exit 2
if [ -s "$memo" ]; then echo warm >> "$0.runs"; else echo cold >> "$0.runs"; fi
printf '%s' 'complete memo fixture' > "$memo"
exit 0
"#,
    )
    .unwrap();
    std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = server.base.join("cache-checker.runs");
    server.configure(serde_json::json!({"flycheck":{
        "enable":false,"command":checker,"cacheDirectory":server.base.join("compiler-cache"),"debounceMs":0
    }}));
    let mut previous_path = None;
    for index in 0..2 {
        server.statuses.clear();
        server.notify("sail-lsp/runFlycheck", serde_json::json!({}));
        server.wait_until("completed project check", |fixture| {
            std::fs::read_to_string(&log).is_ok_and(|text| text.lines().count() == index + 1)
                && fixture
                    .statuses
                    .last()
                    .is_some_and(|s| s.message.as_deref().is_some_and(|m| m.contains(": Passed")))
        });
        let report = server.ask("sail-lsp/diagnosticReport", serde_json::json!({}));
        let history = &report["compilerChecks"][0]["history"];
        assert_eq!(history["lastSuccessCurrent"], true, "{report}");
        assert_eq!(history["cache"]["existingAtStart"], index != 0, "{report}");
        assert!(history["cache"]["bytesAfterCheck"].as_u64().unwrap() > 0);
        let path = history["cache"]["file"].as_str().unwrap().to_owned();
        if let Some(previous) = previous_path {
            assert_eq!(previous, path);
        }
        previous_path = Some(path);
    }
    assert_eq!(std::fs::read_to_string(log).unwrap(), "cold\nwarm\n");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument":{"uri":server.url("a.sail"),"version":2},
            "contentChanges":[{"text":format!("{source}// unsaved input\n")}]
        }),
    );
    let report = server.ask("sail-lsp/diagnosticReport", serde_json::json!({}));
    let history = &report["compilerChecks"][0]["history"];
    assert_eq!(history["lastSuccessCurrent"], false, "{report}");
    assert!(!history["lastSuccess"].is_null(), "past success remains inspectable");
}

#[cfg(unix)]
#[test]
fn flycheck_presentation_updates_without_rechecking_and_hidden_findings_expire_on_edit() {
    let source = "function f() -> int = true\n";
    let mut server = ServerFixture::new_project("compiler-presentation", &[("a.sail", source)]);
    let checker = server.fake_checker(
        "Type error:\na.sail:1.0-8:\n1 |function f() -> int = true\n  |^\n  |compiler-only issue",
    );
    let script = std::fs::read_to_string(&checker).unwrap().replacen("#!/bin/sh\n", "#!/bin/sh\nfor arg do if [ \"$arg\" = --just-check ]; then echo check >> \"$0.runs\"; fi; done\n", 1);
    std::fs::write(&checker, script).unwrap();
    let log = checker.with_file_name("fake-sail.runs");
    let mut config = serde_json::json!({"flycheck":{"enable":false,"command":checker,"cache":"disabled","debounceMs":0}});
    server.configure(config.clone());
    server.did_open("a.sail", source);
    server.notify("sail-lsp/runFlycheck", serde_json::json!({}));
    server.wait_for_diagnostics("a.sail", "native and compiler errors", |diagnostics| {
        diagnostics.iter().any(|d| d.source.as_deref() == Some("sail"))
            && diagnostics.iter().any(|d| {
                d.source.as_deref() != Some("sail")
                    && d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)
            })
    });
    config["flycheck"]["ignore"] = serde_json::json!(["*"]);
    server.configure(config.clone());
    server.wait_for_diagnostics(
        "a.sail",
        "compiler findings hidden without affecting native errors",
        |diagnostics| {
            !diagnostics.iter().any(|d| d.source.as_deref() == Some("sail"))
                && errors(diagnostics) != 0
        },
    );
    assert!(!server.pull("a.sail").iter().any(|d| d.source.as_deref() == Some("sail")));
    config["flycheck"]["ignore"] = serde_json::json!([]);
    config["flycheck"]["warningsAsHint"] = serde_json::json!(["*"]);
    server.configure(config.clone());
    server.wait_for_diagnostics(
        "a.sail",
        "retained compiler finding restored as hint",
        |diagnostics| {
            diagnostics.iter().any(|d| {
                d.source.as_deref() == Some("sail")
                    && d.severity == Some(lsp_types::DiagnosticSeverity::HINT)
            })
        },
    );
    assert!(server.pull("a.sail").iter().any(|d| d.source.as_deref() == Some("sail")
        && d.severity == Some(lsp_types::DiagnosticSeverity::HINT)));
    let report = server.ask("sail-lsp/diagnosticReport", serde_json::json!({}));
    assert_eq!(
        report["compilerChecks"][0]["history"]["lastRun"]["compilerSuccess"], false,
        "presentation cannot turn rejection into success"
    );
    config["flycheck"]["ignore"] = serde_json::json!(["*"]);
    server.configure(config.clone());
    server.notify("sail-lsp/cancelFlycheck", serde_json::json!({}));
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument":{"uri":server.url("a.sail"),"version":2},
            "contentChanges":[{"text":"function f() -> int = 0\n"}]
        }),
    );
    server.wait_for_diagnostics("a.sail", "repaired unsaved input", |diagnostics| {
        diagnostics.is_empty()
    });
    config["flycheck"]["ignore"] = serde_json::json!([]);
    server.configure(config);
    assert!(
        !server.pull("a.sail").iter().any(|d| d.source.as_deref() == Some("sail")),
        "a hidden finding from an older input must not resurrect"
    );
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "check\n",
        "display-only changes do not run the compiler"
    );
}

#[cfg(unix)]
#[test]
fn flycheck_lifecycle_reports_failures_retries_reloads_and_reaps_on_disable_and_shutdown() {
    use std::os::unix::fs::PermissionsExt;
    let mut server =
        ServerFixture::new_project("flycheck-lifecycle", &[("a.sail", "function f() = 0\n")]);
    let checker = server.root.join("checker.sh");
    std::fs::write(
        &checker,
        r#"#!/bin/sh
printf '%s\n' "$*" >> check-runs
case "$(cat check-mode)" in
  fail) echo 'compiler configuration rejected' >&2; exit 2 ;;
  diagnostic) printf 'Type error:\na.sail:1.1-2:\n  | upstream finding\n' >&2; exit 1 ;;
  slow) echo $$ > check-pid; sleep 30 ;;
esac
exit 0
"#,
    )
    .unwrap();
    std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mode = server.root.join("check-mode");
    std::fs::write(&mode, "pass").unwrap();
    let wait_state = |server: &mut ServerFixture, state: &str| {
        server.wait_until(state, |s| {
            s.statuses
                .last()
                .is_some_and(|s| s.message.as_deref().is_some_and(|m| m.contains(state)))
        });
    };
    server.configure(serde_json::json!({"flycheck": {"enable": true, "command": "/missing/sail-binary", "debounceMs": 0}}));
    wait_state(&mut server, "Could not start");
    assert_eq!(server.statuses.last().unwrap().health, crate::lsp_ext::Health::Warning);

    server.configure(
        serde_json::json!({"flycheck": {"enable": true, "command": checker, "debounceMs": 0}}),
    );
    wait_state(&mut server, "Passed"); // Configuration change runs without a save.
    for (mode_value, expected) in [
        ("fail", "compiler configuration rejected"),
        ("diagnostic", "Diagnostics"),
        ("pass", "Passed"),
    ] {
        std::fs::write(&mode, mode_value).unwrap();
        server.statuses.clear();
        server.notify("sail-lsp/recheck", serde_json::Value::Null);
        wait_state(&mut server, expected);
        if mode_value == "diagnostic" {
            server.wait_for_diagnostics("a.sail", "upstream finding", |ds| {
                ds.iter().any(|d| d.source.as_deref() == Some("sail"))
            });
        }
    }
    server.wait_for_diagnostics("a.sail", "successful retry clears upstream diagnostics", |ds| {
        !ds.iter().any(|d| d.source.as_deref() == Some("sail"))
    });
    let runs = std::fs::read_to_string(server.root.join("check-runs")).unwrap().lines().count();
    server.statuses.clear();
    server.create_on_disk("sail.proj", "Main { files a.sail }\n// reload check\n");
    server.wait_until("reload starts a new check without saving a source", |s| {
        std::fs::read_to_string(s.root.join("check-runs")).is_ok_and(|r| r.lines().count() > runs)
    });
    wait_state(&mut server, "Passed");

    std::fs::write(&mode, "slow").unwrap();
    server.statuses.clear();
    server.configure(serde_json::json!({"flycheck": {"enable": true, "command": checker, "debounceMs": 0, "timeoutMs": 3000}}));
    // A loaded runner may skip a displayed second, or deliver several
    // notifications in one pump. No requests or compiler output drive this
    // update; require elapsed progress rather than one exact timestamp.
    server.wait_until("elapsed-time update for a silent compiler", |s| {
        s.statuses.iter().any(|status| {
            !status.quiescent
                && status
                    .message
                    .as_deref()
                    .and_then(|message| message.split_once("Running for "))
                    .and_then(|(_, elapsed)| elapsed.split_once("s."))
                    .and_then(|(seconds, _)| seconds.parse::<u64>().ok())
                    .is_some_and(|seconds| seconds >= 1)
        })
    });
    server.configure(serde_json::json!({"flycheck": {"enable": true, "command": checker, "debounceMs": 0, "timeoutMs": 100}}));
    wait_state(&mut server, "Timed out");
    let assert_reaped = |pid: i32| {
        // SAFETY: signal 0 only queries existence; PID was written by our checker.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "compiler process {pid} survived cancellation"
        );
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    };
    let pid_file = server.root.join("check-pid");
    let read_pid = || std::fs::read_to_string(&pid_file).unwrap().trim().parse::<i32>().unwrap();
    assert_reaped(read_pid());
    std::fs::remove_file(&pid_file).unwrap();
    server.configure(serde_json::json!({"flycheck": {"enable": true, "command": checker, "debounceMs": 0, "timeoutMs": 30_000}}));
    server.wait_until("running compiler before disable", |_| pid_file.is_file());
    let pid = read_pid();
    server.configure(serde_json::json!({"flycheck": {"enable": false}}));
    server.wait_until("disabled Flycheck state cleared", |s| {
        s.statuses.last().is_some_and(|s| {
            s.quiescent && !s.message.as_deref().unwrap_or_default().contains("Flycheck")
        })
    });
    assert_reaped(pid);
    std::fs::remove_file(&pid_file).unwrap();
    server.configure(serde_json::json!({"flycheck": {"enable": true, "command": checker, "debounceMs": 0, "timeoutMs": 30_000}}));
    server.wait_until("running compiler before shutdown", |_| pid_file.is_file());
    let pid = read_pid();
    drop(server);
    assert_reaped(pid);
}

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

/// A file no project lists is checked alone when it is saved. On the native
/// watcher path the save also makes the server rescan the workspace, and the
/// scan began by forgetting every check in flight — the one just started
/// included. With the file open, the check was started over once the scan
/// was done; closed, nothing ran it again, and what upstream said about the
/// file never arrived. A file's own check owes nothing to the project model,
/// so it now runs through the scan and its answer is kept.
#[cfg(unix)]
#[test]
fn a_check_of_an_unlisted_file_survives_the_reload_its_save_triggers() {
    let clean = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_native_watching(
        "flycheck-unlisted-reload",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", clean), ("b.sail", clean)],
    );
    // Slow enough for the scan the save triggers to land while it runs, and
    // deaf to the project's own check, which must not be what answers.
    let checker = server.fake_checker_for(
        Some("b.sail"),
        Duration::from_secs(1),
        "Type error:\n\
         b.sail:3.10-11:\n\
         3 |function f(x : bits(8)) -> bits(8) = x\n  \
           |          ^\n  \
           | int(3) is not a subtype of range(0, 2)",
    );
    server.configure(serde_json::json!({
        "flycheck": { "enable": true, "command": checker.to_string_lossy(), "debounceMs": 0 }
    }));
    server.did_open("b.sail", clean);
    server.wait_for_diagnostics("b.sail", "the unlisted file to be analysed", |d| {
        mentions(d, "not part of a loaded Sail project")
    });
    server.wait_until_scans_settle();

    server.save("b.sail", clean);
    server.wait_for_diagnostics("b.sail", "what upstream said about the unlisted file", |d| {
        d.iter().any(|it| it.source.as_deref() == Some("sail"))
    });
    let runs = |server: &ServerFixture| {
        server.fake_checker_runs().into_iter().filter(|run| run.contains("b.sail")).count()
    };
    assert_eq!(runs(&server), 1, "the one run the save started is the one that answered");

    // Closed, the file has nobody to restart its check after the scan.
    server.did_close("b.sail");
    server.wait_until_scans_settle();
    server.published.remove(&server.url("b.sail"));
    server.save("b.sail", clean);
    server.wait_for_diagnostics("b.sail", "upstream's answer about the closed file", |d| {
        d.iter().any(|it| it.source.as_deref() == Some("sail"))
    });
    assert_eq!(runs(&server), 2);
}

/// Push and pull expose the same current compiler findings; edits invalidate
/// disk-based findings until saved sources can be checked again.
#[cfg(unix)]
#[test]
fn a_pulling_client_is_still_pushed_what_upstream_says() {
    let clean = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_pulling(
        "flycheck-pulling",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", clean)],
    );
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
    server.save("a.sail", clean);
    server.wait_for_diagnostics("a.sail", "what upstream said", |d| {
        !d.is_empty() && d.iter().all(|it| it.source.as_deref() == Some("sail"))
    });
    assert!(server.pull("a.sail").iter().any(|d| d.source.as_deref() == Some("sail")));
    let shifted = format!("// unsaved change\n{clean}");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument":{"uri":server.url("a.sail"),"version":2},
            "contentChanges":[{"text":shifted}]
        }),
    );
    assert!(server.pull("a.sail").iter().all(|d| d.source.as_deref() != Some("sail")));
    server.notify("sail-lsp/recheck", serde_json::Value::Null);
    server.wait_until("unsaved compiler check is deferred", |s| {
        s.statuses.last().is_some_and(|status| {
            status.message.as_deref().unwrap_or_default().contains("Source changed")
        })
    });
    assert!(server.pull("a.sail").iter().all(|d| d.source.as_deref() != Some("sail")));
    server.save("a.sail", &shifted);
    server.wait_for_diagnostics("a.sail", "save restores compiler checks", |d| {
        d.iter().any(|it| it.source.as_deref() == Some("sail"))
    });
    assert!(server.pull("a.sail").iter().any(|d| d.source.as_deref() == Some("sail")));
    server.did_close("a.sail");
    server.did_open("a.sail", clean); // Restored unsaved content, no didChange.
    server.notify("sail-lsp/recheck", serde_json::Value::Null);
    server.wait_until("restored unsaved buffer defers disk checking", |s| {
        s.statuses.last().is_some_and(|status| {
            status.message.as_deref().unwrap_or_default().contains("Source changed")
        })
    });
    assert!(server.pull("a.sail").iter().all(|d| d.source.as_deref() != Some("sail")));
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
    let original = "default Order dec\n\nfunction f(x : bits(8)) -> bits(8) = x\n";
    let mut server = ServerFixture::new_project("tokens", &[("a.sail", original)]);
    server.did_open("a.sail", original);
    let params = serde_json::json!({ "textDocument": { "uri": server.url("a.sail") } });
    let mut baseline = server.ask("textDocument/semanticTokens/full", params.clone());
    assert!(!baseline["data"].as_array().unwrap().is_empty());
    // Exercise growth, deletion, incomplete syntax, non-ASCII offsets and undo.
    let sources = [
        format!("{original}function g(y : bits(4)) -> bits(4) = y\n"),
        "function f(x : int) -> int = if x > 0 then\n".to_owned(),
        "let text = \"中文🙂\"\nfunction f(x : int) -> int = x\n".to_owned(),
        String::new(),
        original.to_owned(),
    ];
    for (version, source) in sources.iter().cycle().take(20).enumerate() {
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": server.url("a.sail"), "version": version + 2},
                "contentChanges": [{"text": source}],
            }),
        );
        let delta = server.ask(
            "textDocument/semanticTokens/full/delta",
            serde_json::json!({
                "textDocument": {"uri": server.url("a.sail")},
                "previousResultId": baseline["resultId"],
            }),
        );
        let mut applied = baseline["data"].as_array().unwrap().clone();
        if let Some(edits) = delta["edits"].as_array() {
            let mut edits = edits.iter().collect::<Vec<_>>();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit["start"].as_u64().unwrap()));
            for edit in edits {
                let start = edit["start"].as_u64().unwrap() as usize;
                let end = start + edit["deleteCount"].as_u64().unwrap() as usize;
                assert!(end <= applied.len(), "invalid delta: {delta}");
                let inserted = edit["data"].as_array().cloned().unwrap_or_default();
                applied.splice(start..end, inserted);
            }
        } else {
            applied = delta["data"].as_array().expect("full fallback").clone();
        }
        let full = server.ask("textDocument/semanticTokens/full", params.clone());
        assert_eq!(&applied, full["data"].as_array().unwrap(), "revision {version}: {source}");
        assert_eq!(applied.len() % 5, 0, "LSP tokens must have five components");
        baseline = full;
    }
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
fn configured_soft_width_reaches_document_and_range_formatting() {
    let source = "function f(x : int) -> int = x + x + x + x + x + x + x + x + x + x + x + x + x + x + x + x + x + x + x + x\n";
    let mut server = ServerFixture::new_project("format-width", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    for method in ["textDocument/formatting", "textDocument/rangeFormatting"] {
        server.configure(serde_json::json!({"formatting": {"lineWidth": 80}}));
        let params = serde_json::json!({
            "textDocument": {"uri": server.url("a.sail")},
            "options": {"tabSize": 2, "insertSpaces": true},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}}
        });
        let edits = server.ask(method, params.clone());
        let formatted = apply_preview(source, &serde_json::json!({"edits":edits}));
        assert!(formatted.contains("-> int =\n  x +"), "{method}: {formatted}");
        server.configure(serde_json::json!({"formatting": {"lineWidth": 200}}));
        let edits = server.ask(method, params);
        assert!(
            edits.is_null() || edits.as_array().is_some_and(Vec::is_empty),
            "{method}: {edits}"
        );
    }
}

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
    assert!(
        !edits.as_array().unwrap().is_empty(),
        "asked to insert a final newline, the server found nothing to do"
    );
    let new_text = apply_preview(no_final_newline, &serde_json::json!({"edits":edits}));
    assert!(
        new_text.ends_with('\n'),
        "the client asked for a final newline and did not get one: {new_text:?}"
    );

    // And what it produced is still the same program.
    server.save("a.sail", &new_text);
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
fn completion_settings_reach_candidates_and_preserve_existing_calls() {
    let source = "function identity(x : int) -> int = x\nfunction identical(x : int) -> int = x\nfunction caller(x : int) -> int = ide\n";
    let mut server = ServerFixture::new_project("completion-settings", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let params = serde_json::json!({
        "textDocument": {"uri": server.url("a.sail")},
        "position": {"line": 2, "character": source.lines().nth(2).unwrap().len()},
    });
    let original = server.ask("textDocument/completion", params.clone());
    let items = original.as_array().expect("completion array");
    assert_eq!(items.len(), 2, "{original}");
    assert!(
        items.iter().all(|item| item["insertText"].as_str().unwrap().contains('(')),
        "{original}"
    );
    server.configure(serde_json::json!({"completion": {
        "limit": 1, "addCallParenthesis": false, "snippets": false,
    }}));
    let configured = server.ask("textDocument/completion", params.clone());
    // Cut at the limit, so the client must ask again as the prefix grows.
    assert_eq!(configured["isIncomplete"], true, "{configured}");
    let items = configured["items"].as_array().expect("completion list");
    assert_eq!(items.len(), 1, "{configured}");
    assert_eq!(items[0]["insertText"], items[0]["label"]);
    server.configure(serde_json::json!({"completion": {
        "limit": 0, "addCallParenthesis": true,
    }}));
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("a.sail"), "version": 2},
            "contentChanges": [{"text": source.replace("= ide\n", "= ide(x)\n")}],
        }),
    );
    let existing_call = server.ask("textDocument/completion", params);
    let items = existing_call.as_array().expect("completion array");
    assert_eq!(items.len(), 2, "{existing_call}");
    for item in items {
        assert_eq!(item["insertText"], item["label"], "must keep the existing (x)");
    }
}

/// A setting the server cannot read is named to the user, once, and costs
/// only itself: the valid setting beside it still applies.
#[test]
fn a_bad_setting_is_reported_once_and_costs_only_itself() {
    let source = "function identity(x : int) -> int = x\nfunction caller(x : int) -> int = ide\n";
    let mut server = ServerFixture::new_project("bad-setting", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let bad = serde_json::json!({"completion": {"limit": "many", "addCallParenthesis": false}});
    server.configure(bad.clone());
    let params = serde_json::json!({
        "textDocument": {"uri": server.url("a.sail")},
        "position": {"line": 1, "character": source.lines().nth(1).unwrap().len()},
    });
    let answer = server.ask("textDocument/completion", params.clone());
    let item = &answer.as_array().expect("completion array")[0];
    assert_eq!(item["insertText"], item["label"], "addCallParenthesis = false still applies");
    assert_eq!(server.messages.len(), 1, "{:?}", server.messages);
    assert!(server.messages[0].message.contains("completion.limit"), "{:?}", server.messages);
    server.configure(bad);
    server.ask("textDocument/completion", params);
    assert_eq!(server.messages.len(), 1, "the same bad setting is not reported again");
}

/// Signature help shows the overload taking as many arguments as the call
/// has, not the one taking the most.
#[test]
fn signature_help_picks_the_overload_the_call_fits() {
    for (call, wanted) in [("f(1)", "f1"), ("f(1, 2, 3)", "f3")] {
        let source = format!(
            "val f3 : (int, int, int) -> int\nval f1 : int -> int\noverload f = {{f3, f1}}\n\
             function g() -> int = {call}\n"
        );
        let mut server = ServerFixture::new_project("signature-overload", &[("a.sail", &source)]);
        server.did_open("a.sail", &source);
        let line = source.lines().nth(3).unwrap();
        let answer = server.ask(
            "textDocument/signatureHelp",
            serde_json::json!({
                "textDocument": {"uri": server.url("a.sail")},
                "position": {"line": 3, "character": line.len() - 1},
            }),
        );
        let active = answer["activeSignature"].as_u64().expect("an active signature") as usize;
        let label = answer["signatures"][active]["label"].as_str().unwrap();
        assert!(label.contains(wanted), "{call}: {answer}");
    }
}

/// The path of an `$include` links to, and goes to, the file it names.
#[test]
fn an_include_path_links_and_goes_to_its_file() {
    let main = "$include \"sub/types.sail\"\nfunction f() -> word = 0x00\n";
    let types = "type word = bits(8)\n";
    let mut server = ServerFixture::new(
        "include-link",
        &[
            ("sail.proj", "Main { files main.sail }"),
            ("main.sail", main),
            ("sub/types.sail", types),
        ],
    );
    server.did_open("main.sail", main);
    let links = server.ask(
        "textDocument/documentLink",
        serde_json::json!({"textDocument": {"uri": server.url("main.sail")}}),
    );
    let link = links
        .as_array()
        .expect("links")
        .iter()
        .find(|l| l["target"].as_str().is_some_and(|t| t.ends_with("/sub/types.sail")))
        .unwrap_or_else(|| panic!("no link to the included file: {links}"));
    assert_eq!(link["range"]["start"], serde_json::json!({"line": 0, "character": 10}));
    assert_eq!(link["range"]["end"], serde_json::json!({"line": 0, "character": 24}));
    let target = server.ask(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "position": {"line": 0, "character": 14},
        }),
    );
    assert!(target.to_string().contains("/sub/types.sail"), "{target}");
}

/// With no limit every candidate arrives, past the 200 that used to be cut
/// silently while the response said the list was complete.
#[test]
fn an_unlimited_completion_list_is_complete() {
    let mut source: String =
        (0..250).map(|i| format!("function cand_{i:03}(x : int) -> int = x\n")).collect();
    source.push_str("function caller(x : int) -> int = cand\n");
    let mut server = ServerFixture::new_project("completion-unlimited", &[("a.sail", &source)]);
    server.did_open("a.sail", &source);
    let params = serde_json::json!({
        "textDocument": {"uri": server.url("a.sail")},
        "position": {"line": 250, "character": source.lines().nth(250).unwrap().len()},
    });
    let limited = server.ask("textDocument/completion", params.clone());
    assert_eq!(limited["isIncomplete"], true, "the default limit cuts 250 to 200");
    assert_eq!(limited["items"].as_array().unwrap().len(), 200);
    server.configure(serde_json::json!({"completion": {"limit": 0}}));
    let all = server.ask("textDocument/completion", params);
    let items = all.as_array().expect("an uncut list is a plain array");
    assert_eq!(
        items.iter().filter(|i| i["label"].as_str().unwrap().starts_with("cand_")).count(),
        250
    );
}

#[test]
fn completion_replaces_identifier_suffix_in_expression_type_field_and_pattern() {
    let cases = [
        ("let banner = \"中文🙂\"\nfunction f(local_value : int) -> int = loc$0TAIL\n", "local_value", "locTAIL"),
        ("struct Record = { value : int }\nfunction f(x : Rec$0TAIL) -> int = 0\n", "Record", "RecTAIL"),
        ("struct S = { first : int, second : bool }\nfunction f(s : S) -> int = s.fi$0TAIL\n", "first", "fiTAIL"),
        ("enum Choice = { First, Second }\nfunction f(c : Choice) -> int = match c { Fi$0TAIL => 1, Second => 2 }\n", "First", "FiTAIL"),
    ];
    for (fixture, label, replaced) in cases {
        let offset = fixture.find("$0").unwrap();
        let source = fixture.replace("$0", "");
        let mut server =
            ServerFixture::new_project("completion-context-edits", &[("a.sail", &source)]);
        server.did_open("a.sail", &source);
        server.configure(
            serde_json::json!({"completion": {"snippets": false, "addCallParenthesis": false}}),
        );
        let index = ide_db::line_index::LineIndex::new(&source);
        let position = crate::to_proto::position_in(&index, offset);
        let answer = server.ask(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": {"uri": server.url("a.sail")}, "position": position,
            }),
        );
        let item = answer
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == label)
            .unwrap_or_else(|| panic!("{fixture}: {answer}"));
        let resolved = server.ask("completionItem/resolve", item.clone());
        assert_eq!(resolved["textEdit"], item["textEdit"]);
        let edit: lsp_types::TextEdit =
            serde_json::from_value(resolved["textEdit"].clone()).unwrap();
        let start = crate::from_proto::offset(&index, edit.range.start);
        let end = crate::from_proto::offset(&index, edit.range.end);
        assert_eq!(&source[start..end], replaced, "{fixture}");
        let mut updated = source.clone();
        updated.replace_range(start..end, &edit.new_text);
        assert_eq!(updated, source.replace(replaced, label));
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": server.url("a.sail"), "version": 2},
                "contentChanges": [{"text": updated}],
            }),
        );
        assert_eq!(errors(&server.pull("a.sail")), 0, "{fixture}");
    }
}

#[test]
fn completion_ranks_a_local_binding_before_a_global_with_the_same_prefix() {
    let source = "let value_global = 1\nfunction f(value_local : int) -> int = value_\n";
    let mut server = ServerFixture::new_project("completion-ranking", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let answer = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": server.url("a.sail")},
            "position": {"line": 1, "character": source.lines().nth(1).unwrap().len()},
        }),
    );
    let items = answer.as_array().unwrap();
    let local = items.iter().find(|item| item["label"] == "value_local").unwrap();
    let global = items.iter().find(|item| item["label"] == "value_global").unwrap();
    assert!(local["sortText"].as_str().unwrap() < global["sortText"].as_str().unwrap(), "{answer}");
}

#[test]
fn function_completion_snippet_applies_with_its_argument_default() {
    let source = "function identity(x : int) -> int = x\nfunction caller(x : int) -> int = ident\n";
    let mut server = ServerFixture::open_with_client(
        "function-snippet",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", source)],
        Root::Real,
        None,
        serde_json::Value::Null,
        ClientProfile { completion_snippets: true, ..Default::default() },
    );
    server.did_open("a.sail", source);
    let answer = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": server.url("a.sail")},
            "position": {"line": 1, "character": source.lines().nth(1).unwrap().len()},
        }),
    );
    let item = answer.as_array().unwrap().iter().find(|item| item["label"] == "identity").unwrap();
    assert_eq!(item["insertTextFormat"], 2);
    let resolved = server.ask("completionItem/resolve", item.clone());
    assert_eq!(resolved["textEdit"], item["textEdit"]);
    let edit: lsp_types::TextEdit = serde_json::from_value(resolved["textEdit"].clone()).unwrap();
    assert_eq!(edit.new_text, "identity(${1:x})");
    let index = ide_db::line_index::LineIndex::new(source);
    let mut updated = source.to_owned();
    updated.replace_range(
        crate::from_proto::offset(&index, edit.range.start)
            ..crate::from_proto::offset(&index, edit.range.end),
        &edit.new_text.replace("${1:x}", "x"),
    );
    assert_eq!(updated, source.replace("= ident\n", "= identity(x)\n"));
    server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": server.url("a.sail"), "version": 2}, "contentChanges": [{"text": updated}],
    }));
    assert_eq!(errors(&server.pull("a.sail")), 0);
}

#[test]
fn postfix_completion_applies_one_replacement_and_survives_resolve() {
    let source = "function f(x : int) -> int = x.ret\n";
    let mut server = ServerFixture::new_project("postfix-edit", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    let answer = server.ask(
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": server.url("a.sail")},
            "position": {"line": 0, "character": source.trim_end().len()},
        }),
    );
    let item = answer.as_array().unwrap().iter().find(|item| item["label"] == ".return").unwrap();
    let resolved = server.ask("completionItem/resolve", item.clone());
    assert_eq!(resolved["textEdit"], item["textEdit"]);
    let edit: lsp_types::TextEdit = serde_json::from_value(resolved["textEdit"].clone()).unwrap();
    let index = ide_db::line_index::LineIndex::new(source);
    let start = crate::from_proto::offset(&index, edit.range.start);
    let end = crate::from_proto::offset(&index, edit.range.end);
    assert_eq!(&source[start..end], "x.ret");
    let mut updated = source.to_owned();
    updated.replace_range(start..end, &edit.new_text);
    assert_eq!(updated, "function f(x : int) -> int = return x\n");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("a.sail"), "version": 2},
            "contentChanges": [{"text": updated}],
        }),
    );
    assert_eq!(errors(&server.pull("a.sail")), 0);
}

#[test]
fn cross_file_rename_updates_signature_implementation_and_calls() {
    for files in [
        [
            ("api.sail", "/* 🙂 */ val helper : int -> int\n"),
            ("impl.sail", "/* 中文🙂 */ function helper(x) = x\n"),
            ("caller.sail", "function caller(x : int) -> int = helper(x)\n"),
        ],
        [
            ("api.sail", "val helper : bool <-> int\n"),
            ("impl.sail", "mapping helper : bool <-> int = { true <-> 1, false <-> 0 }\n"),
            ("caller.sail", "function caller(x : bool) -> int = helper(x)\n"),
        ],
        [
            ("api.sail", "val helper : int -> int\nscattered function helper\n"),
            ("impl.sail", "function clause helper(x) = x\nend helper\n"),
            ("caller.sail", "function caller(x : int) -> int = helper(x)\n"),
        ],
    ] {
        let mut server = ServerFixture::new_project("rename-round-trip", &files);
        for (name, text) in files {
            server.did_open(name, text);
            assert_eq!(errors(&server.pull(name)), 0);
        }
        let at = serde_json::json!({
            "textDocument": {"uri": server.url("caller.sail")},
            "position": {"line": 0, "character": files[2].1.find("helper").unwrap()},
        });
        for (method, expected) in [
            ("textDocument/definition", "impl.sail"),
            ("textDocument/implementation", "impl.sail"),
            ("textDocument/declaration", "api.sail"),
        ] {
            let answer = server.ask(method, at.clone());
            let locations: Vec<lsp_types::Location> = serde_json::from_value(answer).unwrap();
            assert!(!locations.is_empty(), "{method}: {files:?}");
            assert!(
                locations.iter().all(|location| location.uri == server.url(expected)),
                "{method}: {locations:?}"
            );
            for location in locations {
                let text = files.iter().find(|(name, _)| *name == expected).unwrap().1;
                let index = ide_db::line_index::LineIndex::new(text);
                let start = crate::from_proto::offset(&index, location.range.start);
                let end = crate::from_proto::offset(&index, location.range.end);
                assert_eq!(&text[start..end], "helper");
            }
        }
        let mut reference_params = at.clone();
        reference_params["context"] = serde_json::json!({"includeDeclaration": true});
        let references = server.ask("textDocument/references", reference_params);
        let locations: Vec<lsp_types::Location> = serde_json::from_value(references).unwrap();
        let unique: std::collections::HashSet<_> = locations
            .iter()
            .map(|location| {
                (
                    location.uri.clone(),
                    location.range.start.line,
                    location.range.start.character,
                    location.range.end.line,
                    location.range.end.character,
                )
            })
            .collect();
        assert_eq!(locations.len(), unique.len());
        for location in &locations {
            let text = files.iter().find(|(name, _)| server.url(name) == location.uri).unwrap().1;
            let index = ide_db::line_index::LineIndex::new(text);
            let start = crate::from_proto::offset(&index, location.range.start);
            let end = crate::from_proto::offset(&index, location.range.end);
            assert_eq!(&text[start..end], "helper", "{location:?}");
        }

        let mut rename_params = at;
        rename_params["newName"] = serde_json::json!("renamed_helper");
        let edit: lsp_types::WorkspaceEdit =
            serde_json::from_value(server.ask("textDocument/rename", rename_params)).unwrap();
        let Some(lsp_types::DocumentChanges::Edits(documents)) = edit.document_changes else {
            panic!("expected versioned rename edits")
        };
        let changes: std::collections::HashMap<_, Vec<lsp_types::TextEdit>> = documents
            .into_iter()
            .map(|doc| {
                (
                    doc.text_document.uri,
                    doc.edits
                        .into_iter()
                        .map(|e| match e {
                            lsp_types::OneOf::Left(e) => e,
                            _ => panic!("unexpected annotation"),
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(changes.len(), 3, "{changes:?}");
        for (name, text) in files {
            let mut edits = changes[&server.url(name)].clone();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            let index = ide_db::line_index::LineIndex::new(text);
            let mut updated = text.to_owned();
            for edit in edits {
                let start = crate::from_proto::offset(&index, edit.range.start);
                let end = crate::from_proto::offset(&index, edit.range.end);
                updated.replace_range(start..end, &edit.new_text);
            }
            assert_eq!(updated, text.replace("helper", "renamed_helper"));
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url(name), "version": 2},
                    "contentChanges": [{"text": updated}],
                }),
            );
        }
        for (name, _) in files {
            assert_eq!(errors(&server.pull(name)), 0, "{name}");
        }
        let incomplete = if files[1].1.starts_with("mapping") {
            "mapping renamed_helper : bool <-> int = { true <->"
        } else if files[1].1.starts_with("function clause") {
            "function clause renamed_helper(x) ="
        } else {
            "function renamed_helper(x) ="
        };
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument":{"uri":server.url("impl.sail"),"version":3},
                "contentChanges":[{"text":incomplete}]
            }),
        );
        // Recovery may retain an incomplete expression without a hard error.
        // The surviving signature must still support navigation while typing.
        server.pull("impl.sail");
        let declaration = server.ask(
            "textDocument/declaration",
            serde_json::json!({
                "textDocument":{"uri":server.url("caller.sail")},
                "position":{"line":0,"character":files[2].1.find("helper").unwrap()}
            }),
        );
        let declaration: Vec<lsp_types::Location> = serde_json::from_value(declaration).unwrap();
        assert!(
            declaration.iter().any(|location| location.uri == server.url("api.sail")),
            "signature navigation during unfinished editing: {incomplete}"
        );
        for (version, renamed) in [(4, false), (5, true)] {
            for (name, text) in files {
                let text = if renamed {
                    text.replace("helper", "renamed_helper")
                } else {
                    text.to_owned()
                };
                server.notify(
                    "textDocument/didChange",
                    serde_json::json!({
                        "textDocument":{"uri":server.url(name),"version":version},
                        "contentChanges":[{"text":text}]
                    }),
                );
            }
            for (name, _) in files {
                assert_eq!(errors(&server.pull(name)), 0, "undo/redo {version}: {name}");
            }
            let references = server.ask(
                "textDocument/references",
                serde_json::json!({
                    "textDocument":{"uri":server.url("caller.sail")},
                    "position":{"line":0,"character":files[2].1.find("helper").unwrap()},
                    "context":{"includeDeclaration":true}
                }),
            );
            let restored: Vec<lsp_types::Location> = serde_json::from_value(references).unwrap();
            assert_eq!(restored.len(), locations.len(), "undo/redo must restore binding identity");
            for location in restored {
                let text =
                    files.iter().find(|(name, _)| server.url(name) == location.uri).unwrap().1;
                let text = if renamed {
                    text.replace("helper", "renamed_helper")
                } else {
                    text.to_owned()
                };
                let index = ide_db::line_index::LineIndex::new(&text);
                let start = crate::from_proto::offset(&index, location.range.start);
                let end = crate::from_proto::offset(&index, location.range.end);
                assert_eq!(&text[start..end], if renamed { "renamed_helper" } else { "helper" });
            }
        }
    }
}

#[test]
fn closing_delimiter_quick_fixes_apply_at_eof() {
    for source in ["function f() -> int = { 1\n", "function f() -> int = (1 + 2\n"] {
        let mut server =
            ServerFixture::new_project("syntax-fix-roundtrip", &[("main.sail", source)]);
        server.did_open("main.sail", source);
        let diagnostics = server.pull("main.sail");
        assert!(errors(&diagnostics) > 0, "{diagnostics:?}");
        let actions = server.ask("textDocument/codeAction", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": source.lines().count(), "character": 0}},
            "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
        }));
        let action = actions
            .as_array()
            .and_then(|actions| {
                actions.iter().find(|action| {
                    action["title"]
                        .as_str()
                        .is_some_and(|title| title.starts_with("Insert missing"))
                })
            })
            .unwrap_or_else(|| panic!("{source}: {diagnostics:?}: {actions}"));
        let resolved = server.ask("codeAction/resolve", action.clone());
        let edit: lsp_types::WorkspaceEdit =
            serde_json::from_value(resolved["edit"].clone()).unwrap();
        let mut edits = edit.changes.unwrap()[&server.url("main.sail")].clone();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let index = ide_db::line_index::LineIndex::new(source);
        let mut updated = source.to_owned();
        for edit in edits {
            updated.replace_range(
                crate::from_proto::offset(&index, edit.range.start)
                    ..crate::from_proto::offset(&index, edit.range.end),
                &edit.new_text,
            );
        }
        server.notify("textDocument/didChange", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail"), "version": 2}, "contentChanges": [{"text": updated}],
        }));
        let remaining = server.pull("main.sail");
        assert_eq!(errors(&remaining), 0, "{updated}: {remaining:?}");
    }
}

#[test]
fn binding_quick_fixes_remove_their_diagnostic_after_application() {
    for (source, message, title) in [
        (
            "function f(x : int) -> int = { let unused = 42; x + 1 }\n",
            "unused",
            "Rename `unused` to `_unused`",
        ),
        (
            "function f() -> int = { let x = 1; let y = let x = 2 in x; y }\n",
            "unused variable `x`",
            "Rename `x` to `_x`",
        ),
        ("function f() -> int = { return 1; 2 }\n", "unreachable", "Remove unreachable code"),
    ] {
        let mut server =
            ServerFixture::new_project("binding-fix-roundtrip", &[("main.sail", source)]);
        server.did_open("main.sail", source);
        let diagnostics = server.pull("main.sail");
        let target = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.to_lowercase().contains(message))
            .unwrap_or_else(|| panic!("missing {message}: {diagnostics:?}"));
        let code = target.code.clone().expect("diagnostic code");
        let actions = server.ask(
            "textDocument/codeAction",
            serde_json::json!({
                "textDocument": {"uri": server.url("main.sail")}, "range": target.range,
                "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
            }),
        );
        let action = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["title"] == title)
            .unwrap_or_else(|| panic!("missing {title}: {actions}"));
        let resolved = server.ask("codeAction/resolve", action.clone());
        let edit: lsp_types::WorkspaceEdit =
            serde_json::from_value(resolved["edit"].clone()).unwrap();
        let changes = edit.changes.unwrap();
        let mut edits = changes[&server.url("main.sail")].clone();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let index = ide_db::line_index::LineIndex::new(source);
        let mut updated = source.to_owned();
        for edit in edits {
            let start = crate::from_proto::offset(&index, edit.range.start);
            let end = crate::from_proto::offset(&index, edit.range.end);
            updated.replace_range(start..end, &edit.new_text);
        }
        if source.contains("let y = let x") {
            assert_eq!(
                updated,
                source.replacen("let x", "let _x", 1),
                "the fix must leave the used inner binding unchanged"
            );
        }
        server.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": server.url("main.sail"), "version": 2},
                "contentChanges": [{"text": updated}],
            }),
        );
        let stale = server.request("codeAction/resolve", action.clone());
        let response = server.wait_for_response(stale);
        assert_eq!(response.error.unwrap().code, lsp_server::ErrorCode::ContentModified as i32);
        let remaining = server.pull("main.sail");
        assert_eq!(errors(&remaining), 0, "{updated}: {remaining:?}");
        assert!(
            remaining.iter().all(|diagnostic| diagnostic.code.as_ref() != Some(&code)),
            "{remaining:?}"
        );
    }
}

#[test]
fn every_record_fill_action_uses_instantiated_cross_file_types() {
    let types = "struct S('n : Int) = { x : int, data : bits('n), enabled : bool }\nstruct Other = { x : int, unrelated : string }\n";
    for literal in ["struct S { x = 1 }", "struct { x = 1, }"] {
        let source = format!("function f() -> S(8) = {literal}\n");
        let mut server = ServerFixture::new_project(
            "record-fill-roundtrip",
            &[("types.sail", types), ("main.sail", &source)],
        );
        server.did_open("main.sail", &source);
        let diagnostics = server.pull("main.sail");
        let at = source.find("x =").unwrap();
        let actions = server.ask("textDocument/codeAction", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "range": {"start": {"line": 0, "character": at}, "end": {"line": 0, "character": at}},
            "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
        }));
        let fills = actions
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| {
                action["title"].as_str().is_some_and(|title| {
                    title.starts_with("Add missing field")
                        || title.starts_with("Fill missing field")
                })
            })
            .collect::<Vec<_>>();
        assert!(!fills.is_empty(), "{literal}: {actions}");
        for (index, action) in fills.iter().enumerate() {
            let resolved = server.ask("codeAction/resolve", (*action).clone());
            let edit: lsp_types::WorkspaceEdit =
                serde_json::from_value(resolved["edit"].clone()).unwrap();
            let changes = edit.changes.expect("field edits");
            let mut edits = changes[&server.url("main.sail")].clone();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            let line_index = ide_db::line_index::LineIndex::new(&source);
            let mut updated = source.clone();
            for edit in edits {
                let start = crate::from_proto::offset(&line_index, edit.range.start);
                let end = crate::from_proto::offset(&line_index, edit.range.end);
                updated.replace_range(start..end, &edit.new_text);
            }
            assert!(updated.contains("data = 0x00"), "{updated}");
            assert!(updated.contains("enabled = false"), "{updated}");
            assert!(!updated.contains("unrelated"), "must select S, not Other");
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 2},
                    "contentChanges": [{"text": updated}],
                }),
            );
            let remaining = server.pull("main.sail");
            assert_eq!(errors(&remaining), 0, "{updated}: {remaining:?}");
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 3},
                    "contentChanges": [{"text": source}],
                }),
            );
        }
    }
}

#[test]
fn record_pattern_fill_preserves_cross_file_generic_match_coverage() {
    let types = "struct S('n : Int) = { x : int, data : bits('n), enabled : bool }\nstruct Other = { x : int, unrelated : string }\n";
    for literal in ["struct { x = n }", "struct { x = n, }"] {
        let source = format!("function f(s : S(8)) -> int = match s {{ {literal} => n }}\n");
        let mut server = ServerFixture::new_project(
            "record-pattern-roundtrip",
            &[("types.sail", types), ("main.sail", &source)],
        );
        server.did_open("main.sail", &source);
        let diagnostics = server.pull("main.sail");
        let at = source.find("x =").unwrap();
        let actions = server.ask("textDocument/codeAction", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "range": {"start": {"line": 0, "character": at}, "end": {"line": 0, "character": at}},
            "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
        }));
        let fills = actions
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| {
                action["title"].as_str().is_some_and(|title| {
                    title.starts_with("Add missing field")
                        || title.starts_with("Fill missing field")
                })
            })
            .collect::<Vec<_>>();
        assert!(!fills.is_empty(), "{literal}: {actions}");
        for (index, action) in fills.iter().enumerate() {
            let resolved = server.ask("codeAction/resolve", (*action).clone());
            let edit: lsp_types::WorkspaceEdit =
                serde_json::from_value(resolved["edit"].clone()).unwrap();
            let changes = edit.changes.expect("field edits");
            let mut edits = changes[&server.url("main.sail")].clone();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            let line_index = ide_db::line_index::LineIndex::new(&source);
            let mut updated = source.clone();
            for edit in edits {
                let start = crate::from_proto::offset(&line_index, edit.range.start);
                let end = crate::from_proto::offset(&line_index, edit.range.end);
                updated.replace_range(start..end, &edit.new_text);
            }
            assert!(updated.contains("data = _"), "{updated}");
            assert!(updated.contains("enabled = _"), "{updated}");
            assert!(!updated.contains("unrelated"), "must select S, not Other");
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 2},
                    "contentChanges": [{"text": updated}],
                }),
            );
            let remaining = server.pull("main.sail");
            assert_eq!(errors(&remaining), 0, "{updated}: {remaining:?}");
            assert!(
                !remaining.iter().any(|d| d.code
                    == Some(lsp_types::NumberOrString::String("incomplete-match".into()))),
                "{remaining:?}"
            );
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 3},
                    "contentChanges": [{"text": source}],
                }),
            );
        }
    }
}

#[test]
fn match_fixes_apply_and_recheck_cross_file_constructor_payloads() {
    let types = "union U = { Some : bool, None : unit }\nenum Other = X | Y\n";
    for literal in [
        "match c { Some(true) => 1, None => 0 }",
        "match c { Some(true) => 1, None => 0, /* } */ }",
    ] {
        let source = format!("function f(c : U) -> int = {literal}\n");
        let mut server = ServerFixture::new_project(
            "match-fill-roundtrip",
            &[("types.sail", types), ("main.sail", &source)],
        );
        server.did_open("main.sail", &source);
        let diagnostics = server.pull("main.sail");
        let at = source.find("match").unwrap();
        let actions = server.ask("textDocument/codeAction", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "range": {"start": {"line": 0, "character": at}, "end": {"line": 0, "character": at}},
            "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
        }));
        let fills = actions
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| {
                action["title"].as_str().is_some_and(|title| {
                    title.starts_with("Fill ") || title.starts_with("Add wildcard arm")
                })
            })
            .collect::<Vec<_>>();
        assert!(!fills.is_empty(), "{literal}: {actions}");
        for (index, action) in fills.iter().enumerate() {
            let resolved = server.ask("codeAction/resolve", (*action).clone());
            let edit: lsp_types::WorkspaceEdit =
                serde_json::from_value(resolved["edit"].clone()).unwrap();
            let changes = edit.changes.expect("field edits");
            let mut edits = changes[&server.url("main.sail")].clone();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            let line_index = ide_db::line_index::LineIndex::new(&source);
            let mut updated = source.clone();
            for edit in edits {
                let start = crate::from_proto::offset(&line_index, edit.range.start);
                let end = crate::from_proto::offset(&line_index, edit.range.end);
                updated.replace_range(start..end, &edit.new_text);
            }
            assert!(updated.contains("undefined"), "{updated}");
            assert!(!updated.contains("todo("), "{updated}");
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 2},
                    "contentChanges": [{"text": updated}],
                }),
            );
            let remaining = server.pull("main.sail");
            assert_eq!(errors(&remaining), 0, "{updated}: {remaining:?}");
            assert!(
                !remaining.iter().any(|d| d.code
                    == Some(lsp_types::NumberOrString::String("incomplete-match".into()))),
                "{updated}: {remaining:?}"
            );
            server.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail"), "version": index * 2 + 3},
                    "contentChanges": [{"text": source}],
                }),
            );
        }
    }
}

fn inlay_label(hint: &serde_json::Value) -> String {
    if let Some(label) = hint["label"].as_str() {
        label.to_owned()
    } else {
        hint["label"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part["value"].as_str().unwrap())
            .collect()
    }
}

#[test]
fn inlay_links_reach_the_client_with_utf16_source_locations() {
    let definitions = "/*😀*/ struct Widget = { count : int }\n/*😀*/ function build(seed : int) -> Widget = struct { count = seed }\n";
    let main = "function use() -> int = { let item = build(4); item.count }\n";
    let mut server = ServerFixture::new_project(
        "inlay-navigation",
        &[("defs.sail", definitions), ("main.sail", main)],
    );
    server.did_open("main.sail", main);
    let hints = server.ask(
        "textDocument/inlayHint",
        serde_json::json!({
            "textDocument":{"uri":server.url("main.sail")},
            "range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}}
        }),
    );
    for (label, target) in [(": Widget", "Widget"), ("seed:", "seed")] {
        let hint =
            hints.as_array().unwrap().iter().find(|hint| inlay_label(hint) == label).unwrap();
        let part = hint["label"]
            .as_array()
            .unwrap()
            .iter()
            .find(|part| !part["location"].is_null())
            .unwrap();
        let before = &definitions[..definitions.find(target).unwrap()];
        assert_eq!(part["location"]["uri"], server.url("defs.sail").to_string());
        assert_eq!(
            part["location"]["range"]["start"],
            serde_json::json!({
                "line":before.matches('\n').count(),
                "character":before.rsplit('\n').next().unwrap().encode_utf16().count()
            })
        );
        let resolved = server.ask("inlayHint/resolve", hint.clone());
        assert_eq!(resolved["label"], hint["label"], "resolve preserves navigation");
    }
}

#[test]
fn inlay_parameter_names_and_length_settings_reach_the_client() {
    let source = "val pick : int -> int\nfunction pick(first_parameter) = first_parameter\nfunction before() -> int = pick(1)\nval pick : int -> int\nfunction pick(second_parameter) = second_parameter\nfunction after() -> int = pick(2)\n";
    let mut server = ServerFixture::new_project("inlay-scope-settings", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let params = serde_json::json!({
        "textDocument": {"uri": server.url("main.sail")},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 6, "character": 0}},
    });
    let full = server.ask("textDocument/inlayHint", params.clone());
    let labels = full
        .as_array()
        .unwrap()
        .iter()
        .filter(|hint| hint["kind"] == 2)
        .map(inlay_label)
        .collect::<Vec<_>>();
    assert_eq!(labels, ["first_parameter:", "second_parameter:"]);
    server.configure(serde_json::json!({"inlayHints": {"maxLength": 6}}));
    let short = server.ask("textDocument/inlayHint", params);
    let hints =
        short.as_array().unwrap().iter().filter(|hint| hint["kind"] == 2).collect::<Vec<_>>();
    assert_eq!(hints[1]["label"], "secon…");
    let resolved = server.ask("inlayHint/resolve", hints[1].clone());
    assert!(resolved["tooltip"].to_string().contains("second_parameter:"));
}

#[test]
fn hover_overloads_are_bounded_and_respect_visible_member_signatures() {
    let declarations = "val option_a : int -> int\nval option_b : int -> int\nval option_c : int -> int\noverload choose = { option_a, option_b, option_c }\n";
    let source = "function caller(x : int) -> int = choose(x)\n";
    let mut server = ServerFixture::new_project(
        "hover-overload-limit",
        &[
            ("api.sail", declarations),
            ("main.sail", source),
            ("later.sail", "val option_a : bool -> bool\n"),
        ],
    );
    server.did_open("main.sail", source);
    for limit in [serde_json::json!(1), serde_json::Value::Null] {
        server.configure(serde_json::json!({"hover": {"maxOverloadsCount": limit}}));
        let hover = server.ask(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": server.url("main.sail")},
                "position": {"line": 0, "character": source.find("choose").unwrap()},
            }),
        );
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("option_a"), "{markdown}");
        assert!(!markdown.contains("bool"), "invisible replacement: {markdown}");
        assert_eq!(markdown.contains("option_c"), limit.is_null(), "{markdown}");
        if !limit.is_null() {
            assert!(markdown.contains("+2 more"), "{markdown}");
        }
    }
}

#[test]
fn hover_member_limits_apply_to_titles_and_code_blocks() {
    let source = "struct Large = { first : vector(2, bits(8)), second : (int, bool), hidden : string }\nenum Color = { Red, Green, Blue }\n";
    let mut server = ServerFixture::new_project("hover-member-limits", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    for limit in [serde_json::json!(1), serde_json::Value::Null, serde_json::json!(0)] {
        server.configure(
            serde_json::json!({"hover": {"maxFieldsCount": limit, "maxEnumVariantsCount": limit}}),
        );
        for (line, character, hidden) in [(0, 9, "hidden"), (1, 6, "Blue")] {
            let hover = server.ask(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": {"uri": server.url("main.sail")},
                    "position": {"line": line, "character": character},
                }),
            );
            let markdown = hover["contents"]["value"].as_str().unwrap();
            if limit.is_null() {
                assert!(markdown.contains(hidden), "{markdown}");
            } else {
                assert!(!markdown.contains(hidden), "{markdown}");
                assert!(markdown.contains("more)"), "{markdown}");
            }
        }
    }
}

#[test]
fn hover_signatures_exclude_replaced_cross_file_declarations() {
    let before = "function before() -> int = choose(1)\n";
    let after = "function after() -> bool = choose(true)\n";
    let mut server = ServerFixture::new_project(
        "hover-selected-signature",
        &[
            ("old.sail", "val choose : int -> int\n"),
            ("before.sail", before),
            ("new.sail", "val choose : bool -> bool\n"),
            ("after.sail", after),
        ],
    );
    for (file, source, expected, excluded) in
        [("before.sail", before, "int", "bool"), ("after.sail", after, "bool", "int")]
    {
        server.did_open(file, source);
        let hover = server.ask(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": server.url(file)},
                "position": {"line": 0, "character": source.find("choose").unwrap() + 2},
            }),
        );
        let markdown = hover["contents"]["value"].as_str().expect("hover markdown");
        assert!(markdown.contains(expected), "{markdown}");
        assert!(!markdown.contains(excluded), "invisible signature in {file}: {markdown}");
        assert!(!markdown.contains("overloads:"), "{markdown}");
    }
}

#[test]
fn inlay_parameters_use_cross_file_implementation_identity() {
    let declarations = "val pick : int -> int\nval external_only : int -> int\n";
    let implementation = "function pick(arg1) = arg1\n";
    let calls =
        "function before() -> int = pick(1)\nfunction external_call() -> int = external_only(1)\n";
    let replacement = "val pick : int -> int\nfunction pick(replacement) = replacement\n";
    let after = "function after() -> int = pick(2)\n";
    let mut server = ServerFixture::new_project(
        "inlay-cross-file-identity",
        &[
            ("decl.sail", declarations),
            ("impl.sail", implementation),
            ("calls.sail", calls),
            ("replacement.sail", replacement),
            ("after.sail", after),
        ],
    );
    for (file, source, expected) in
        [("calls.sail", calls, "arg1:"), ("after.sail", after, "replacement:")]
    {
        server.did_open(file, source);
        let hints = server.ask(
            "textDocument/inlayHint",
            serde_json::json!({
                "textDocument": {"uri": server.url(file)},
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}},
            }),
        );
        let labels = hints
            .as_array()
            .unwrap()
            .iter()
            .filter(|hint| hint["kind"] == 2)
            .map(inlay_label)
            .collect::<Vec<_>>();
        assert_eq!(labels, [expected], "{file}: {hints}");
    }
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
        let mut server = ServerFixture::open_with_options(
            if imports { "auto-include-resolve" } else { "auto-include-quickfix" },
            &[
                ("sail.proj", "Main { files src/main.sail }"),
                ("src/main.sail", main),
                ("lib/helper.sail", "/// helper docs\nval helper : unit -> unit\n"),
            ],
            Root::Real,
            Some(imports),
            serde_json::json!({"completion": {"autoimport": true}}),
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

#[test]
fn extracting_an_include_creates_a_file_and_keeps_semantic_order() {
    let selected = "val helper : int -> int\nfunction helper(x) = x\n";
    let source = format!("{selected}function caller() -> int = helper(1)\n");
    let mut server = ServerFixture::new_project("extract-include", &[("main.sail", &source)]);
    server.did_open("main.sail", &source);
    let params = serde_json::json!({
        "textDocument": {"uri": server.url("main.sail")},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}},
        "context": {"diagnostics": [], "only": ["refactor.extract"]},
    });
    let actions = server.ask("textDocument/codeAction", params.clone());
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Extract to helper_extracted.sail")
        .unwrap_or_else(|| panic!("{actions}"))
        .clone();
    let resolved = server.ask("codeAction/resolve", action.clone());
    let operations = resolved["edit"]["documentChanges"].as_array().unwrap();
    assert_eq!(operations.len(), 3, "{resolved}");
    assert_eq!(operations[0]["kind"], "create");
    assert_eq!(operations[0]["uri"], server.url("helper_extracted.sail").as_str());
    assert_eq!(operations[0]["options"]["overwrite"], false);
    assert_eq!(operations[0]["options"]["ignoreIfExists"], false);
    assert_eq!(operations[1]["edits"][0]["newText"], selected);
    assert_eq!(operations[2]["textDocument"]["version"], 1);
    assert_eq!(operations[2]["edits"][0]["newText"], "$include \"helper_extracted.sail\"");
    // A collision after offering the action must fail during resolve as well.
    server.create_on_disk("helper_extracted.sail", selected);
    let collision = server.request("codeAction/resolve", action.clone());
    assert_eq!(
        server.wait_for_response(collision).error.unwrap().code,
        lsp_server::ErrorCode::ContentModified as i32
    );
    let updated = "$include \"helper_extracted.sail\"\nfunction caller() -> int = helper(1)\n";
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail"), "version": 2},
            "contentChanges": [{"text": updated}],
        }),
    );
    // An old empty publication can still be queued before didChange. Wait
    // for the new file to be part of the model, then check current diagnostics.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let locations = server.ask("textDocument/definition", serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")}, "position": {"line": 1, "character": 32},
        }));
        if locations.as_array().is_some_and(|locations| {
            locations
                .iter()
                .any(|location| location["uri"] == server.url("helper_extracted.sail").as_str())
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "extracted include never loaded: {locations}");
        std::thread::sleep(Duration::from_millis(10));
    }
    let diagnostics = server.pull("main.sail");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    server.wait_for_diagnostics("main.sail", "extracted include to remain linked", |ds| {
        ds.is_empty()
    });
    let stale = server.request("codeAction/resolve", action);
    assert_eq!(
        server.wait_for_response(stale).error.unwrap().code,
        lsp_server::ErrorCode::ContentModified as i32
    );
}

#[test]
fn source_actions_preserve_includes_used_only_by_types() {
    let source = "$include \"types.sail\"\nval identity : T -> T\nfunction identity(x) = x\n";
    let mut server = ServerFixture::new_project(
        "include-type-dependency",
        &[("types.sail", "enum T = A | B\n"), ("main.sail", source)],
    );
    server.did_open("main.sail", source);
    let diagnostics = server.pull("main.sail");
    assert_eq!(errors(&diagnostics), 0, "{diagnostics:?}");
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "context": {"diagnostics": diagnostics, "only": ["source"]},
        }),
    );
    assert!(
        actions.is_null()
            || actions.as_array().is_some_and(|actions| !actions
                .iter()
                .any(|action| action["title"]
                    .as_str()
                    .is_some_and(|title| title.contains("unused imports")))),
        "{actions}"
    );
}

#[test]
fn stale_client_diagnostic_cannot_rename_a_now_used_binding() {
    let source = "function f(x : int) -> int = { let value = 42; x + 1 }\n";
    let mut server =
        ServerFixture::new_project("stale-client-diagnostic", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let diagnostics = server.pull("main.sail");
    let target = diagnostics
        .iter()
        .find(|d| d.code == Some(lsp_types::NumberOrString::String("unused-variable".into())))
        .unwrap();
    let updated = source.replace("x + 1", "value + x");
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail"), "version": 2},
            "contentChanges": [{"text": updated}],
        }),
    );
    let actions = server.ask(
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")}, "range": target.range,
            "context": {"diagnostics": diagnostics, "only": ["quickfix"]},
        }),
    );
    assert!(
        actions.is_null()
            || actions.as_array().is_some_and(|actions| !actions
                .iter()
                .any(|action| action["title"]
                    .as_str()
                    .is_some_and(|title| title.starts_with("Rename `value`")))),
        "{actions}"
    );
    assert_eq!(errors(&server.pull("main.sail")), 0);
}

#[test]
fn local_rename_preserves_shadowed_global_and_non_code_occurrences() {
    let source = "let value : int = 1\nfunction f() -> int = { let value : int = 2; value }\nfunction g() -> int = value\n// value stays in this comment\nlet text : string = \"value\"\n";
    let mut server = ServerFixture::new_project("rename-shadow-identity", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    assert_eq!(errors(&server.pull("main.sail")), 0);
    let offset = source.find("value }").unwrap();
    let index = ide_db::line_index::LineIndex::new(source);
    let position = crate::to_proto::position_in(&index, offset);
    let edit: lsp_types::WorkspaceEdit = serde_json::from_value(server.ask(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")}, "position": position,
            "newName": "local_value",
        }),
    ))
    .unwrap();
    let Some(lsp_types::DocumentChanges::Edits(documents)) = edit.document_changes else {
        panic!("versioned rename expected")
    };
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].text_document.uri, server.url("main.sail"));
    assert_eq!(documents[0].text_document.version, Some(1));
    let mut edits: Vec<lsp_types::TextEdit> = documents
        .into_iter()
        .next()
        .unwrap()
        .edits
        .into_iter()
        .map(|e| match e {
            lsp_types::OneOf::Left(e) => e,
            _ => panic!("unexpected annotation"),
        })
        .collect();
    assert_eq!(edits.len(), 2, "{edits:?}");
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut updated = source.to_owned();
    for edit in edits {
        let start = crate::from_proto::offset(&index, edit.range.start);
        let end = crate::from_proto::offset(&index, edit.range.end);
        updated.replace_range(start..end, &edit.new_text);
    }
    assert_eq!(
        updated,
        source.replace("let value : int = 2; value", "let local_value : int = 2; local_value")
    );
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": server.url("main.sail"), "version": 2},
            "contentChanges": [{"text": updated}],
        }),
    );
    assert_eq!(errors(&server.pull("main.sail")), 0);
}

#[test]
fn analyzer_status_explains_the_running_build_and_loaded_project() {
    let mut server = ServerFixture::new_project(
        "analyzer-status-provenance",
        &[("main.sail", "function f() -> int = 1\n")],
    );
    let status = server.ask("sail-lsp/analyzerStatus", serde_json::Value::Null);
    let status = status.as_str().expect("status text");
    assert!(status.contains(&crate::version::version()), "{status}");
    assert!(status.contains(&std::env::current_exe().unwrap().display().to_string()), "{status}");
    assert!(status.contains(server.root.to_str().unwrap()), "{status}");
    assert!(status.contains("sail.proj"), "{status}");
    assert!(status.contains("SAIL_DIR environment"), "{status}");
    assert!(status.contains("workspace loaded: true"), "{status}");
    assert!(status.contains("\"diagnostics\""), "{status}");
    server.notify(
        "workspace/didChangeConfiguration",
        serde_json::json!({
            "settings": {"sail-lsp": {"completion": {"limit": 37}}}
        }),
    );
    let updated = server.ask("sail-lsp/analyzerStatus", serde_json::Value::Null);
    assert!(updated.as_str().unwrap().contains("\"limit\": 37"), "{updated}");
}

#[test]
fn analyzer_status_exposes_load_errors_and_effective_fallback_settings() {
    let mut server = ServerFixture::open_with_options(
        "analyzer-status-error",
        &[("broken.sail_project", "Main { files missing.sail }")],
        Root::Real,
        None,
        serde_json::json!({"diagnostics": {"enable": "invalid"}}),
    );
    let status = server.ask("sail-lsp/analyzerStatus", serde_json::Value::Null);
    let status = status.as_str().unwrap();
    assert!(status.contains("missing.sail"), "{status}");
    assert!(status.contains("configuration error"), "{status}");
    assert!(status.contains("\"enable\": true"), "{status}");
}

#[test]
fn signature_help_uses_types_for_equal_arity_overloads() {
    for (argument, wanted) in [("true", "on_bool"), ("42", "on_int"), ("value", "on_bool")] {
        let source = format!("val on_int : int -> int\nval on_bool : bool -> int\noverload pick_value = {{on_int, on_bool}}\nfunction f(value : bool) -> int = pick_value({argument})\n");
        let mut server = ServerFixture::new_project("signature-types", &[("main.sail", &source)]);
        server.did_open("main.sail", &source);
        let answer = server.ask(
            "textDocument/signatureHelp",
            serde_json::json!({
                "textDocument": {"uri": server.url("main.sail")},
                "position": {"line": 3, "character": source.lines().nth(3).unwrap().len()-1}
            }),
        );
        let index = answer["activeSignature"].as_u64().expect("active signature") as usize;
        assert!(
            answer["signatures"][index]["label"].as_str().unwrap().contains(wanted),
            "{answer}"
        );
    }
}

#[test]
fn ssr_previews_all_matches_and_respects_bindings_selections_and_versions() {
    let defs = "val combine : (int, int) -> int\nval flipped : (int, int) -> int\n";
    let main = "function f() -> int = combine(1, 2)\nfunction g() -> int = combine(3, 4)\n";
    let other = "function h() -> int = combine(5, 6)\n";
    let shadow = "function local(combine : (int, int) -> int) -> int = combine(7, 8)\n";
    let mut server = ServerFixture::new_project(
        "ssr-preview",
        &[("defs.sail", defs), ("main.sail", main), ("other.sail", other), ("shadow.sail", shadow)],
    );
    server.did_open("main.sail", main);
    server.did_open("other.sail", other);
    let params = serde_json::json!({
        "query": "combine($a, $b) ==>> flipped($b, $a)", "parseOnly": false,
        "textDocument": {"uri": server.url("main.sail")}, "position": {"line": 0, "character": 23}, "selections": []
    });
    let preview = server.ask("experimental/ssr", params.clone());
    let changes = preview["documentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{preview}");
    assert_eq!(
        changes.iter().map(|c| c["edits"].as_array().unwrap().len()).sum::<usize>(),
        3,
        "{preview}"
    );
    let mut applied = Vec::new();
    for change in changes {
        assert_eq!(change["textDocument"]["version"], 1, "{change}");
        let rel = if change["textDocument"]["uri"] == server.url("main.sail").as_str() {
            "main.sail"
        } else {
            "other.sail"
        };
        let source = if rel == "main.sail" { main } else { other };
        let mut edits: Vec<lsp_types::TextEdit> =
            serde_json::from_value(change["edits"].clone()).unwrap();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let index = ide_db::line_index::LineIndex::new(source);
        let mut updated = source.to_owned();
        for edit in edits {
            updated.replace_range(
                crate::from_proto::offset(&index, edit.range.start)
                    ..crate::from_proto::offset(&index, edit.range.end),
                &edit.new_text,
            );
        }
        assert!(
            updated.contains(if rel == "main.sail" {
                "flipped((2), (1))"
            } else {
                "flipped((6), (5))"
            }),
            "{updated}"
        );
        applied.push((rel, updated));
    }
    let mut selected = params.clone();
    selected["selections"] =
        serde_json::json!([{"start":{"line":1,"character":0},"end":{"line":2,"character":0}}]);
    let selected = server.ask("experimental/ssr", selected);
    assert_eq!(selected["documentChanges"].as_array().unwrap().len(), 1, "{selected}");
    assert_eq!(selected["documentChanges"][0]["edits"].as_array().unwrap().len(), 1, "{selected}");
    let mut parse_only = params.clone();
    parse_only["parseOnly"] = true.into();
    assert_eq!(
        server.ask("experimental/ssr", parse_only)["documentChanges"],
        serde_json::json!([])
    );
    for rule in [
        "combine($a, $b) ==>> flipped($c, $a)",
        "combine($a, $b) ==>> (",
        "combine($a, $b) ==>> absent($a)",
    ] {
        let mut invalid = params.clone();
        invalid["query"] = rule.into();
        let request = server.request("experimental/ssr", invalid);
        let response = server.wait_for_response(request);
        assert_eq!(response.error.expect(rule).code, lsp_server::ErrorCode::InvalidParams as i32);
    }
    for (rel, updated) in applied {
        server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url(rel),"version":2},"contentChanges":[{"text":updated}]}));
        assert_eq!(errors(&server.pull(rel)), 0, "{updated}");
    }
}

#[test]
fn branch_like_manifest_renames_and_generated_batches_recover() {
    let caller = "function caller(x : int) -> int = identity(x)\n";
    let valid = "function identity(x : int) -> int = x\n";
    let invalid = "function identity(x : bool) -> bool = x\n";
    let mut server = ServerFixture::new_project(
        "branch-generated-batch",
        &[("old.sail", valid), ("caller.sail", caller)],
    );
    server.did_open("caller.sail", caller);
    assert_eq!(errors(&server.pull("caller.sail")), 0);
    let definition = |server: &mut ServerFixture| {
        server.ask("textDocument/definition", serde_json::json!({
        "textDocument":{"uri":server.url("caller.sail")},"position":{"line":0,"character":34}
    }))
    };
    assert!(definition(&mut server).to_string().contains("old.sail"));
    for round in 0..3 {
        // The disk transaction models checkout/code generation: rename the
        // provider, replace the manifest and create a batch of generated files.
        let (old, new) =
            if round % 2 == 0 { ("old.sail", "new.sail") } else { ("new.sail", "old.sail") };
        std::fs::rename(server.path(old), server.path(new)).unwrap();
        std::fs::write(server.path(new), invalid).unwrap();
        let mut paths = vec![new.to_owned(), "caller.sail".to_owned()];
        let mut events = vec![
            serde_json::json!({"uri":server.url(old),"type":3}),
            serde_json::json!({"uri":server.url(new),"type":1}),
        ];
        for n in 0..16 {
            let path = format!("generated_{n}.sail");
            std::fs::write(
                server.path(&path),
                format!("function generated_{n}() -> int = {round}\n"),
            )
            .unwrap();
            events.push(
                serde_json::json!({"uri":server.url(&path),"type":if round == 0 {1} else {2}}),
            );
            paths.push(path);
        }
        std::fs::write(server.path("sail.proj"), format!("Main {{ files {} }}", paths.join(", ")))
            .unwrap();
        events.push(serde_json::json!({"uri":server.url("sail.proj"),"type":2}));
        server.notify("workspace/didChangeWatchedFiles", serde_json::json!({"changes":events}));
        server.wait_for_diagnostics("caller.sail", "new branch changes provider signature", |d| {
            errors(d) > 0
        });
        let moved = definition(&mut server).to_string();
        assert!(moved.contains(new) && !moved.contains(old), "{moved}");
        // Sustained edits in two buffers while a request is cancelled.
        server.did_open(new, invalid);
        for version in 2..12 {
            for (path, text) in [
                (new, if version % 2 == 0 { "function identity(" } else { invalid }),
                ("caller.sail", caller),
            ] {
                server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url(path),"version":round*20+version},"contentChanges":[{"text":text}]}));
            }
        }
        let pending = server.request(
            "textDocument/documentSymbol",
            serde_json::json!({"textDocument":{"uri":server.url(new)}}),
        );
        server.notify("$/cancelRequest", serde_json::json!({"id":pending}));
        let response = server.wait_for_response(pending);
        if let Some(error) = response.error {
            assert!(matches!(error.code, -32800 | -32801), "{error:?}");
        }
        server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url(new),"version":round*20+12},"contentChanges":[{"text":valid}]}));
        server.wait_for_diagnostics(
            "caller.sail",
            "valid unsaved provider clears stale diagnostics",
            |d| errors(d) == 0,
        );
        assert_eq!(errors(&server.pull(new)), 0);
        std::fs::write(server.path(new), valid).unwrap();
        server.close_url(&server.url(new));
        server.wait_until("server becomes quiescent after burst", |f| {
            f.statuses.last().is_some_and(|s| s.quiescent)
        });
    }
    let mut events = Vec::new();
    for n in 0..16 {
        let path = format!("generated_{n}.sail");
        std::fs::remove_file(server.path(&path)).unwrap();
        events.push(serde_json::json!({"uri":server.url(&path),"type":3}));
    }
    std::fs::write(server.path("sail.proj"), "Main { files new.sail, caller.sail }").unwrap();
    events.push(serde_json::json!({"uri":server.url("sail.proj"),"type":2}));
    server.log.clear();
    server.notify("workspace/didChangeWatchedFiles", serde_json::json!({"changes":events}));
    server.wait_until("generated files leave workspace", |f| {
        f.log.iter().any(|m| m.contains("background scan: 2 files"))
    });
    let symbols = server.ask("workspace/symbol", serde_json::json!({"query":"generated_"}));
    assert!(symbols.is_null() || symbols == serde_json::json!([]), "{symbols}");
    assert_eq!(errors(&server.pull("caller.sail")), 0);
}

#[test]
fn conversion_quick_fixes_apply_and_recheck_over_the_protocol() {
    for wrapper in ["unsigned", "signed"] {
        let source = "default Order dec\nval unsigned : bits(8) -> int\nval signed : bits(8) -> int\nfunction f(v : bits(8)) -> int = v\n";
        let mut server =
            ServerFixture::new_project("conversion-roundtrip", &[("main.sail", source)]);
        server.did_open("main.sail", source);
        let diagnostics = server.pull("main.sail");
        let target = diagnostics
            .iter()
            .find(|d| d.code == Some(lsp_types::NumberOrString::String("type-error".into())))
            .unwrap();
        let actions = server.ask("textDocument/codeAction", serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"range":target.range,"context":{"diagnostics":diagnostics,"only":["quickfix"]}}));
        let title = format!("Wrap with `{wrapper}(...)`");
        let action = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == title)
            .unwrap_or_else(|| panic!("{actions}"));
        let resolved = server.ask("codeAction/resolve", action.clone());
        let edit: lsp_types::WorkspaceEdit =
            serde_json::from_value(resolved["edit"].clone()).unwrap();
        let mut edits = edit.changes.unwrap()[&server.url("main.sail")].clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let index = ide_db::line_index::LineIndex::new(source);
        let mut updated = source.to_owned();
        for edit in edits {
            updated.replace_range(
                crate::from_proto::offset(&index, edit.range.start)
                    ..crate::from_proto::offset(&index, edit.range.end),
                &edit.new_text,
            );
        }
        server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url("main.sail"),"version":2},"contentChanges":[{"text":updated}]}));
        assert_eq!(errors(&server.pull("main.sail")), 0, "{updated}");
    }
}

#[test]
fn ssr_preserves_precedence_comments_and_repeated_capture_constraints() {
    for (expr, query, count, replacement) in [
        ("combine(1 + 2, 3) * 4", "combine($a, $b) ==>> $a + $b", 1, "((1 + 2) + (3)) * 4"),
        ("combine(1, 1)", "combine(${a:kind(literal)}, $a) ==>> $a", 1, "((1))"),
        ("combine(1, 2)", "combine($a, $a) ==>> $a", 0, ""),
        ("combine(1 + 2, 1 + 2)", "combine(${a:kind(literal)}, $a) ==>> $a", 0, ""),
        ("combine(/* keep */ 1, 2)", "combine($a, $b) ==>> $a", 0, ""),
        ("combine((1 /* keep */ + 2), 3)", "combine($a, $b) ==>> $a", 1, "keep"),
        ("combine(combine(1, 2), 3)", "combine($a, $b) ==>> $a", 1, "combine(1, 2)"),
    ] {
        let source = format!("val combine : (int, int) -> int\nfunction f() -> int = {expr}\n");
        let mut server = ServerFixture::new_project("ssr-syntax-safety", &[("main.sail", &source)]);
        server.did_open("main.sail", &source);
        let result = server.ask("experimental/ssr", serde_json::json!({"query":query,"textDocument":{"uri":server.url("main.sail")},"position":{"line":1,"character":22},"selections":[]}));
        let changes = result["documentChanges"].as_array().unwrap();
        let edits: Vec<lsp_types::TextEdit> = changes
            .iter()
            .flat_map(|c| {
                serde_json::from_value::<Vec<lsp_types::TextEdit>>(c["edits"].clone()).unwrap()
            })
            .collect();
        assert_eq!(edits.len(), count, "{query}: {result}");
        let index = ide_db::line_index::LineIndex::new(&source);
        let mut updated = source.clone();
        for edit in edits {
            updated.replace_range(
                crate::from_proto::offset(&index, edit.range.start)
                    ..crate::from_proto::offset(&index, edit.range.end),
                &edit.new_text,
            );
        }
        if count > 0 {
            assert!(updated.contains(replacement), "{updated}");
        }
        server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url("main.sail"),"version":2},"contentChanges":[{"text":updated}]}));
        assert_eq!(errors(&server.pull("main.sail")), 0, "{updated}");
    }
}

#[test]
fn signature_help_distinguishes_dependent_widths_and_aliases() {
    for (argument, wanted) in [("0x12", "byte"), ("0x1234", "word"), ("value", "word")] {
        let source = format!("default Order dec\ntype word_bits = bits(16)\nval byte : bits(8) -> int\nval word : bits(16) -> int\noverload pick_width = {{byte, word}}\nfunction f(value : word_bits) -> int = pick_width({argument})\n");
        let mut server = ServerFixture::new_project("signature-widths", &[("main.sail", &source)]);
        server.did_open("main.sail", &source);
        let answer = server.ask("textDocument/signatureHelp", serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":5,"character":source.lines().nth(5).unwrap().len()-1}}));
        let index = answer["activeSignature"].as_u64().expect("active signature") as usize;
        assert!(
            answer["signatures"][index]["label"].as_str().unwrap().contains(wanted),
            "{answer}"
        );
    }
}

#[test]
fn ssr_selects_nested_matches_and_checks_shared_include_contexts() {
    {
        let source =
            "val combine : (int, int) -> int\nfunction f() -> int = combine(combine(1, 2), 3)\n";
        let mut server =
            ServerFixture::new_project("ssr-nested-selection", &[("main.sail", source)]);
        let start = source.lines().nth(1).unwrap().find("combine(1").unwrap();
        let result = server.ask("experimental/ssr", serde_json::json!({"query":"combine($a, $b) ==>> $b","textDocument":{"uri":server.url("main.sail")},"position":{"line":1,"character":start},"selections":[{"start":{"line":1,"character":start},"end":{"line":1,"character":start+13}}]}));
        assert_eq!(result["documentChanges"][0]["edits"][0]["newText"], "((2))", "{result}");
    }
    let mut server = ServerFixture::new(
        "ssr-shared-include",
        &[
            ("sail.proj", "A { files a.sail } B { files b.sail }"),
            ("a.sail", "$include \"shared.sail\"\n"),
            ("b.sail", "$include \"shared.sail\"\n"),
            ("shared.sail", "function shared() -> int = 1\n"),
        ],
    );
    let request = server.request("experimental/ssr", serde_json::json!({"query":"1 ==>> 2","textDocument":{"uri":server.url("shared.sail")},"position":{"line":0,"character":27},"selections":[]}));
    let response = server.wait_for_response(request);
    assert!(response.error.is_none(), "same literal and same edit in both contexts: {response:?}");
    let change = response.result.unwrap();
    assert_eq!(change["documentChanges"].as_array().unwrap().len(), 1);
    assert_eq!(change["documentChanges"][0]["edits"].as_array().unwrap().len(), 1);
}

#[test]
fn ssr_does_not_edit_inactive_preprocessor_branches() {
    let source = "$ifdef NEVER_DEFINED\nfunction inactive() -> int = 1\n$else\nfunction active() -> int = 1\n$endif\n";
    let mut server = ServerFixture::new_project("ssr-active-branches", &[("main.sail", source)]);
    let result = server.ask("experimental/ssr", serde_json::json!({"query":"1 ==>> 2","textDocument":{"uri":server.url("main.sail")},"position":{"line":3,"character":27},"selections":[]}));
    let edits = result["documentChanges"][0]["edits"].as_array().expect("one active file");
    assert_eq!(edits.len(), 1, "{result}");
    assert_eq!(edits[0]["range"]["start"]["line"], 3, "{result}");
}

#[test]
fn linked_projects_select_shared_file_context_without_unloading_other_projects() {
    let a = "variable ALT = false\nLib { files if $ALT then wrong.sail else a.sail } Main { requires Lib files shared.sail } Inactive {}\n";
    let b = "Lib { files b.sail } Main { requires Lib files shared.sail }\n";
    let shared = "function use() -> int = chosen()\n";
    let settings = |active: &str| {
        serde_json::json!({"workspace": {
            "linkedProjects": [{"projectFile": "a.sail_project", "projectVariables": {"ALT": false}, "projectModules": ["Main"]}, "b.sail_project"],
            "activeProject": active
        }})
    };
    let mut server = ServerFixture::open_with_options(
        "linked-contexts",
        &[
            ("a.sail_project", a),
            ("b.sail_project", b),
            ("shared.sail", shared),
            ("a.sail", "function chosen() -> int = 1\n"),
            ("b.sail", "function chosen() -> int = 2\n"),
        ],
        Root::Real,
        None,
        settings("a.sail_project"),
    );
    server.did_open("shared.sail", shared);
    let definition = |s: &mut ServerFixture| {
        s.ask("textDocument/definition", serde_json::json!({"textDocument": {"uri": s.url("shared.sail")}, "position": {"line": 0, "character": 25}}))
    };
    assert!(definition(&mut server).to_string().contains("a.sail"));
    let explain = |s: &mut ServerFixture| {
        s.ask("sail-lsp/explainAnalysis", serde_json::json!({"textDocument": {"uri": s.url("shared.sail")}, "position": {"line": 0, "character": 25}}))
    };
    let context = explain(&mut server);
    assert_eq!(context["contexts"][0]["project"]["module"], "Main", "{context}");
    assert_eq!(context["contexts"][0]["project"]["variables"]["ALT"], false, "{context}");
    assert_eq!(context["contexts"][0]["project"]["requires"][0], "Lib", "{context}");
    let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
    assert_eq!(graph.as_array().unwrap().len(), 2);
    assert_eq!(graph[0]["variables"]["ALT"], false);
    assert_eq!(graph[0]["modules"][1]["requires"][0], "Lib");
    assert_eq!(graph[0]["modules"][2]["active"], false);
    server.configure(settings("b.sail_project"));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !definition(&mut server).to_string().contains("b.sail") {
        assert!(Instant::now() < deadline, "shared file did not switch context");
        server.pump(Duration::from_millis(20));
    }
    let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
    assert_eq!(graph.as_array().unwrap().len(), 2);
    assert!(graph[0]["path"].as_str().unwrap().ends_with("b.sail_project"));
    let context = explain(&mut server);
    assert!(
        context["contexts"][0]["project"]["manifest"].as_str().unwrap().ends_with("b.sail_project"),
        "{context}"
    );
    server.create_on_disk("a.sail_project", "Broken { requires Missing }");
    server.wait_for_diagnostics("a.sail_project", "independent broken project", |d| {
        mentions(d, "Missing")
    });
    assert!(
        server.log.iter().any(|message| message.contains("project reload: 1 scanned, 1 reused")),
        "a linked-project manifest edit should not rescan its neighbour: {:?}",
        server.log
    );
    assert!(definition(&mut server).to_string().contains("b.sail"));
    let context = explain(&mut server);
    assert!(context["projectLoadError"].as_str().unwrap().contains("Missing"), "{context}");
    assert!(
        context["contexts"][0]["project"]["manifest"].as_str().unwrap().ends_with("b.sail_project"),
        "{context}"
    );
    assert_eq!(
        server.ask("sail-lsp/projectGraph", serde_json::Value::Null).as_array().unwrap().len(),
        2
    );
    server.configure(settings("absent.sail_project"));
    server.wait_until("invalid active project explains its fallback", |s| {
        s.statuses.last().is_some_and(|s| {
            s.message.as_deref().is_some_and(|m| {
                m.contains("workspace.activeProject") && m.contains("first loaded manifest")
            })
        })
    });
    assert!(definition(&mut server).to_string().contains("a.sail"));
}

#[test]
fn ssr_search_returns_nested_semantic_locations_without_edits() {
    let defs = "val combine : (int, int) -> int\n";
    let main = "function f() -> int = /*😀*/ combine(1, combine(2, 3))\n";
    let other = "function h() -> int = combine(4, 5)\n";
    let shadow = "function local(combine : (int, int) -> int) -> int = combine(7, 8)\n";
    let mut server = ServerFixture::new_project(
        "ssr-search",
        &[("defs.sail", defs), ("main.sail", main), ("other.sail", other), ("shadow.sail", shadow)],
    );
    server.did_open("main.sail", main);
    let column = |offset| main[..offset].encode_utf16().count() as u32;
    let mut params = serde_json::json!({
        "query":"combine($a, $b)", "textDocument":{"uri":server.url("main.sail")},
        "position":{"line":0,"character":column(main.find("combine").unwrap())}, "selections":[]
    });
    let found = server.ask("sail-lsp/ssrSearch", params.clone());
    let locations = found.as_array().unwrap();
    assert_eq!(locations.len(), 3, "{found}");
    assert!(locations.iter().all(|loc| loc["uri"] != server.url("shadow.sail").as_str()));
    let inner_start = column(main.find("combine(2").unwrap());
    let inner_end = inner_start + "combine(2, 3)".len() as u32;
    params["selections"] = serde_json::json!([{"start":{"line":0,"character":inner_start},"end":{"line":0,"character":inner_end}}]);
    let selected = server.ask("sail-lsp/ssrSearch", params.clone());
    assert_eq!(selected.as_array().unwrap().len(), 1, "{selected}");
    assert_eq!(selected[0]["range"]["start"]["character"], inner_start);
    assert_eq!(selected[0]["range"]["end"]["character"], inner_end);
    assert_eq!(std::fs::read_to_string(server.root.join("main.sail")).unwrap(), main);
    server.did_change_appending("main.sail", 1, "function unfinished( =\n");
    let incomplete = server.ask("sail-lsp/ssrSearch", params.clone());
    assert_eq!(incomplete.as_array().unwrap().len(), 1, "{incomplete}");
    params["query"] = serde_json::json!("combine($a, $b) ==>> $a");
    let id = server.request("sail-lsp/ssrSearch", params);
    let response = server.wait_for_response(id);
    assert_eq!(response.error.unwrap().code, lsp_server::ErrorCode::InvalidParams as i32);
}

#[test]
fn ssr_search_deduplicates_physical_locations_across_include_views() {
    let main =
        "$include \"shared.sail\"\n$include \"shared.sail\"\nfunction origin() -> int = 12345\n";
    let shared = "function shared_value() -> int = 12345\n";
    let mut server = ServerFixture::new_project(
        "ssr-search-views",
        &[("main.sail", main), ("shared.sail", shared)],
    );
    server.did_open("main.sail", main);
    let found = server.ask(
        "sail-lsp/ssrSearch",
        serde_json::json!({
            "query":"12345", "textDocument":{"uri":server.url("main.sail")},
            "position":{"line":2,"character":27}, "selections":[]
        }),
    );
    let locations = found.as_array().unwrap();
    assert_eq!(locations.len(), 2, "{found}");
    assert_eq!(
        locations
            .iter()
            .filter(|location| location["uri"] == server.url("shared.sail").as_str())
            .count(),
        1,
        "{found}"
    );
}

/// Check advertised capabilities through the protocol, independently of how
/// request types and dispatcher registrations are arranged in source files.
#[test]
fn every_experimental_capability_has_a_handler() {
    use crate::lsp_ext::{
        IncludeContexts, JoinLines, MatchingBrace, MoveItem, OnEnter, Ssr, SsrPreview, SsrSearch,
    };
    use lsp_types::request::Request as _;

    let source = "function answer() -> int = 42\n";
    let mut server = ServerFixture::new_project("experimental-handlers", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let experimental = server.capabilities["experimental"].as_object().unwrap().clone();
    let methods = [
        ("ssr", Ssr::METHOD),
        ("ssrSearch", SsrSearch::METHOD),
        ("ssrPreview", SsrPreview::METHOD),
        ("includeContexts", IncludeContexts::METHOD),
        ("diagnosticReport", crate::lsp_ext::DiagnosticReport::METHOD),
        ("matchingBrace", MatchingBrace::METHOD),
        ("joinLines", JoinLines::METHOD),
        ("onEnter", OnEnter::METHOD),
        ("moveItem", MoveItem::METHOD),
    ];
    for (name, enabled) in experimental {
        if enabled != serde_json::Value::Bool(true) {
            continue;
        }
        let (_, method) = methods
            .iter()
            .find(|(capability, _)| *capability == name)
            .unwrap_or_else(|| panic!("no protocol probe for advertised capability `{name}`"));
        let id = server.request(method, serde_json::json!({
            "textDocument": {"uri": server.url("main.sail")},
            "position": {"line": 0, "character": 27},
            "positions": [{"line": 0, "character": 15}],
            "ranges": [], "selections": [],
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 28}},
            "direction": "Up", "parseOnly": true, "uri":server.url("main.sail"),
            "query": if name == "ssrSearch" { "42" } else { "42 ==>> 43" }
        }));
        let response = server.wait_for_response(id);
        assert!(response.error.is_none(), "{name} ({method}): {:?}", response.error);
    }
}

#[test]
fn protocol_distinguishes_unknown_methods_and_invalid_parameters() {
    let mut server = ServerFixture::new("protocol-errors", &[]);
    let id = server.request("sail-lsp/nonexistent", serde_json::json!({}));
    let response = server.wait_for_response(id);
    assert_eq!(response.error.unwrap().code, lsp_server::ErrorCode::MethodNotFound as i32);
    let id = server.request("textDocument/hover", serde_json::json!({"position": false}));
    let response = server.wait_for_response(id);
    assert_eq!(response.error.unwrap().code, lsp_server::ErrorCode::InvalidParams as i32);
    // A malformed request must not poison the next request on the connection.
    let id = server.request("sail-lsp/projectGraph", serde_json::Value::Null);
    assert!(server.wait_for_response(id).error.is_none());
}

#[test]
fn file_moves_update_only_resolved_paths_and_use_buffer_versions() {
    let source = "$include \"a/util.sail\" // \"util.sail\"\n$include \"b/util.sail\"\nlet text = \"util.sail\"\n";
    let mut server = ServerFixture::new(
        "path-rename",
        &[
            ("main.sail", source),
            ("a/util.sail", "val a : int\n"),
            ("b/util.sail", "val b : int\n"),
            ("sail.proj", "Main { files main.sail, a/util.sail, b/util.sail }"),
        ],
    );
    server.did_open("main.sail", source);
    server.did_open("sail.proj", "Main { files main.sail, a/util.sail, b/util.sail }");
    let edit = server.ask(
        "workspace/willRenameFiles",
        serde_json::json!({"files":[
            {"oldUri":server.url("a"),"newUri":server.url("moved")},
            {"oldUri":server.url("main.sail"),"newUri":server.url("b/main.sail")}
        ]}),
    );
    let changes = edit["documentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{edit}");
    let main = changes
        .iter()
        .find(|d| d["textDocument"]["uri"] == server.url("main.sail").as_str())
        .unwrap();
    assert_eq!(main["textDocument"]["version"], 1);
    let replacements: Vec<_> =
        main["edits"].as_array().unwrap().iter().map(|e| e["newText"].as_str().unwrap()).collect();
    assert_eq!(replacements, ["../moved/util.sail", "util.sail"]);
    assert_eq!(main["edits"][0]["range"]["start"]["line"], 0);
    assert_eq!(main["edits"][1]["range"]["start"]["line"], 1);
    let manifest = changes
        .iter()
        .find(|d| d["textDocument"]["uri"] == server.url("sail.proj").as_str())
        .unwrap();
    assert_eq!(manifest["textDocument"]["version"], 1);
    let edits = manifest["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2, "{edit}");
    assert_eq!(edits[0]["newText"], "\"b/main.sail\"");
    assert_eq!(edits[1]["newText"], "\"moved/util.sail\"");
}

#[test]
fn workspace_symbol_subsequences_filters_and_identifier_ranges() {
    let mut server = ServerFixture::new_project("symbol-query", &[("main.sail",
        "val memory_read : int -> int\nfunction memory_read(x : int) -> int = x\nstruct MachineRegister = { value : int }\n")]);
    let result = server.ask(
        "workspace/symbol",
        serde_json::json!({"query":"mr kind:function scope:workspace limit:1"}),
    );
    let symbols = result.as_array().unwrap();
    assert_eq!(symbols.len(), 1, "{result}");
    assert_eq!(symbols[0]["name"], "memory_read");
    assert_eq!(
        symbols[0]["location"]["range"]["start"],
        serde_json::json!({"line":1,"character":9})
    );
    assert_eq!(symbols[0]["location"]["range"]["end"]["character"], 20);
    assert!(symbols[0]["containerName"].as_str().unwrap().contains("Main"));
    let result = server.ask("workspace/symbol", serde_json::json!({"query":"#mr"}));
    assert_eq!(result[0]["name"], "MachineRegister", "{result}");
    assert!(server
        .ask("workspace/symbol", serde_json::json!({"query":"mr scope:library"}))
        .is_null());
}

#[test]
fn include_context_selection_changes_reads_but_keeps_shared_edits_unsafe() {
    let shared = "function use(x : int) -> int = helper(x)\n";
    let mut server = ServerFixture::new(
        "context-choice",
        &[
            ("sail.proj", "A { files a.sail } B { files b.sail }"),
            ("a.sail", "val helper : int -> int\n$include \"shared.sail\"\n"),
            ("b.sail", "val helper : bool -> bool\n$include \"shared.sail\"\n"),
            ("shared.sail", shared),
        ],
    );
    server.did_open("shared.sail", shared);
    let contexts = server
        .ask("sail-lsp/includeContexts", serde_json::json!({"uri":server.url("shared.sail")}));
    let contexts = contexts.as_array().unwrap();
    assert_eq!(contexts.len(), 2, "{contexts:?}");
    let selected = contexts.iter().find(|c| c["project"]["module"] == "B").unwrap()["key"].clone();
    server.ask(
        "sail-lsp/selectIncludeContext",
        serde_json::json!({"textDocument":{"uri":server.url("shared.sail")},"context":selected}),
    );
    let position = serde_json::json!({"textDocument":{"uri":server.url("shared.sail")},"position":{"line":0,"character":32}});
    let hover = server.ask("textDocument/hover", position.clone());
    assert!(hover.to_string().contains("bool"), "{hover}");
    let id = server.request(
        "experimental/ssr",
        serde_json::json!({"textDocument":{"uri":server.url("shared.sail")},
        "position":{"line":0,"character":32},"query":"$x ==>> $x","parseOnly":false}),
    );
    let response = server.wait_for_response(id);
    assert!(response.error.unwrap().message.contains("multiple include"));
    server.did_change_appending("shared.sail", 0, "// inserted before the function\n");
    let hover=server.ask("textDocument/hover",serde_json::json!({"textDocument":{"uri":server.url("shared.sail")},"position":{"line":1,"character":32}}));
    assert!(hover.to_string().contains("bool"), "{hover}");
    server.did_close("shared.sail");
    server.did_open("shared.sail", shared);
    let contexts = server
        .ask("sail-lsp/includeContexts", serde_json::json!({"uri":server.url("shared.sail")}));
    assert!(contexts.as_array().unwrap().iter().all(|c| c["selected"] == false));
}

#[test]
fn project_graph_notifies_after_installing_saved_model() {
    let mut server = ServerFixture::new_project("graph-refresh", &[("main.sail", "val x : int\n")]);
    let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
    assert_eq!(graph[0]["modules"].as_array().unwrap().len(), 1);
    let baseline = server.graph_changes;
    let manifest = "Main { files main.sail } Extra { }";
    server.did_open("sail.proj", manifest);
    let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
    assert_eq!(graph[0]["modules"].as_array().unwrap().len(), 1);
    assert_eq!(server.graph_changes, baseline, "unsaved editor text must not install a graph");
    server.save("sail.proj", manifest);
    server.wait_until("graph changed notification", |s| s.graph_changes > baseline);
    let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
    assert_eq!(graph[0]["modules"].as_array().unwrap().len(), 2, "{graph}");
}

fn apply_preview(source: &str, change: &serde_json::Value) -> String {
    let mut edits: Vec<lsp_types::TextEdit> =
        serde_json::from_value(change["edits"].clone()).unwrap();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let index = ide_db::line_index::LineIndex::new(source);
    let mut output = source.to_owned();
    for edit in edits {
        output.replace_range(
            crate::from_proto::offset(&index, edit.range.start)
                ..crate::from_proto::offset(&index, edit.range.end),
            &edit.new_text,
        );
    }
    output
}

#[test]
fn ssr_supports_semantic_type_item_and_binding_fragments() {
    for (name, source, query, expected) in [
        (
            "types",
            "val convert : bitvector(8) -> bitvector(8)\n",
            "type: bitvector($n) ==>> vector($n, bool)",
            "vector(8, bool)",
        ),
        (
            "items",
            "val convert : bitvector(8) -> bitvector(8)\n",
            "item: val $f : bitvector($n) -> bitvector($n) ==>> val $f : bitvector($n) -> int",
            "val convert : bitvector(8) -> int",
        ),
        (
            "let-bindings",
            "function f() -> int = let x = 1 in x\n",
            "let x = $v in x ==>> let x = $v in x + 1",
            "sail_ssr_x_0",
        ),
        (
            "captured-bindings",
            "function f() -> int = let x = 1 in x + 2\n",
            "let $x = $v in $body ==>> let $x = $v in $body + 1",
            "x + 2",
        ),
        (
            "patterns",
            "function f() -> int = let (x, y) = (1, 2) in x\n",
            "pattern: ($a, $b) ==>> ($b, $a)",
            "(y, x)",
        ),
    ] {
        let mut server =
            ServerFixture::new_project(&format!("ssr-fragment-{name}"), &[("main.sail", source)]);
        server.did_open("main.sail", source);
        let value=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":0},"query":query}));
        let changes = value["documentChanges"].as_array().unwrap();
        assert_eq!(changes.len(), 1, "{name}: {value}");
        let after = apply_preview(source, &changes[0]);
        assert!(after.contains(expected), "{name}: {after}");
        assert!(syntax::parse_text(&after).1.is_empty(), "{name}: {after}");
    }
}

#[test]
fn ssr_type_identity_does_not_cross_unrelated_module_aliases() {
    let a = "type Word = int\nval a : Word\n";
    let b = "type Word = bool\nval b : Word\n";
    let mut server = ServerFixture::new(
        "ssr-type-identity",
        &[("sail.proj", "A { files a.sail } B { files b.sail }"), ("a.sail", a), ("b.sail", b)],
    );
    server.did_open("a.sail", a);
    let value=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("a.sail")},"position":{"line":1,"character":8},"query":"type: Word ==>> int"}));
    let changes = value["documentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "{value}");
    assert_eq!(changes[0]["textDocument"]["uri"], server.url("a.sail").as_str());
}

#[test]
fn ssr_hygiene_does_not_capture_outer_variables_or_let_bindings_escape() {
    let source = "function f(x : int) -> int = x + 1\nfunction g() -> int = let x = 2 in x + 1\n";
    let mut server = ServerFixture::new_project("ssr-hygiene", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let params = serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":28},"query":"$body + 1 ==>> let x = 0 in $body + x"});
    let value = server.ask("experimental/ssr", params);
    let after = apply_preview(source, &value["documentChanges"][0]);
    assert!(after.contains("sail_ssr_x_0"), "{after}");
    assert!(after.contains("(x) + sail_ssr_x_0"), "outer x must remain free: {after}");
    let value=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":28},"query":"let $x = $value in $body ==>> $body"}));
    assert!(
        value["documentChanges"].as_array().unwrap().is_empty(),
        "bound x would escape: {value}"
    );
}

#[test]
fn ssr_captured_binding_names_cannot_escape_or_be_deleted_from_patterns() {
    let source = "function f() -> int = let x = 1 in x\n";
    let mut server = ServerFixture::new_project("ssr-binder-escape", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    for query in ["let $x = $v in $x ==>> $x", "pattern: $p ==>> _"] {
        let value=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":0},"query":query}));
        assert!(value["documentChanges"].as_array().unwrap().is_empty(), "{query}: {value}");
    }
}

#[test]
fn ssr_fixed_item_names_resolve_to_the_selected_declaration() {
    let source = "type Word = int\nval value : Word\n";
    let mut server = ServerFixture::new(
        "ssr-item-identity",
        &[
            ("sail.proj", "A { files a.sail } B { files b.sail }"),
            ("a.sail", source),
            ("b.sail", source),
        ],
    );
    server.did_open("a.sail", source);
    for line in [0, 1] {
        let result=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("a.sail")},
            "position":{"line":line,"character":0},"query":"item: type Word = int ==>> type Word = bool"}));
        let changes = result["documentChanges"].as_array().unwrap();
        assert_eq!(changes.len(), 1, "{result}");
        assert_eq!(changes[0]["textDocument"]["uri"], server.url("a.sail").as_str());
    }
}

#[test]
fn ssr_quantified_types_and_nested_expression_scopes_remain_hygienic() {
    for (name, source, query) in [
        (
            "quantifier",
            "val f : forall ('n : Int). bitvector('n) -> bitvector('n)\n",
            "type: bitvector($n) ==>> {'n, 'n >= 0. ($n, bitvector('n))}",
        ),
        (
            "match",
            "function f(x : int) -> int = match x { y => y }\n",
            "match $value { y => $body } ==>> match $value { y => $body + 1 }",
        ),
        (
            "foreach",
            "function f() -> unit = foreach (i from 0 to 3) { let x = i; () }\n",
            "foreach (i from 0 to $end) $body ==>> foreach (i from 0 to $end) $body",
        ),
    ] {
        let mut server =
            ServerFixture::new_project(&format!("ssr-hygiene-{name}"), &[("main.sail", source)]);
        server.did_open("main.sail", source);
        let result=server.ask("experimental/ssr",serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":0},"query":query}));
        let changes = result["documentChanges"].as_array().unwrap();
        assert!(!changes.is_empty(), "{name}: {result}");
        let after = apply_preview(source, &changes[0]);
        assert!(syntax::parse_text(&after).1.is_empty(), "{name}: {after}");
        assert!(after.contains("sail_ssr_"), "{name}: {after}");
    }
}

#[test]
fn stale_include_selection_returns_an_intentional_protocol_error() {
    let source = "val x : int\n";
    let mut server = ServerFixture::new_project("stale-include-choice", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let id = server.request(
        "sail-lsp/selectIncludeContext",
        serde_json::json!({"textDocument":{"uri":server.url("main.sail")},"context":"expired-key"}),
    );
    let error = server.wait_for_response(id).error.unwrap();
    assert_eq!(error.code, lsp_server::ErrorCode::InvalidParams as i32);
    assert!(error.message.contains("no longer active"));
}

#[test]
fn diagnostic_report_contains_effective_context_and_no_source_text() {
    let source = "function report_target() -> int = 42\n";
    let mut server = ServerFixture::new_project("diagnostic-report", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let report = server.ask(
        "sail-lsp/diagnosticReport",
        serde_json::json!({"textDocument":{"uri":server.url("main.sail")}}),
    );
    assert_eq!(report["schemaVersion"], 1);
    assert!(report["build"]["stdlibRevision"].is_string());
    assert_eq!(report["workspace"]["loaded"], true);
    assert_eq!(report["activeFile"]["uri"], server.url("main.sail").as_str());
    assert!(report["nativeAnalysis"]["failures"].is_object());
    assert!(report["effectiveConfiguration"]["workspace"].is_object());
    assert!(
        !serde_json::to_string(&report).unwrap().contains(source.trim()),
        "reports do not contain source buffers"
    );
    std::fs::write(server.root.join("sail.proj"), "Main { files").unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({"changes":[{"uri":server.url("sail.proj"),"type":2}]}),
    );
    server.wait_until("invalid manifest is visible", |s| {
        s.statuses.last().is_some_and(|status| status.health == crate::lsp_ext::Health::Error)
    });
    let report = server.ask("sail-lsp/diagnosticReport", serde_json::json!({}));
    assert!(report["workspace"]["loadError"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(report["serverStatus"]["health"], "error");
}

#[test]
fn shared_refactor_protocol_deduplicates_edits_rechecks_and_rejects_stale_actions() {
    // Both includes contribute clauses to one scattered function. The stock
    // upstream compiler accepts this project; ordinary duplicate definitions
    // would already be invalid before any edit.
    let shared = "\nfunction clause f(x) = { let saved = x; saved }\n";
    let a = "val f : int -> int\nscattered function f\n$include \"shared.sail\"\n";
    let b = "$include \"shared.sail\"\nend f\n";
    let binding = shared.lines().nth(1).unwrap().find("x)").unwrap();
    let mut server = ServerFixture::new(
        "shared-safe-actions",
        &[
            ("sail.proj", "A { files a.sail } B { requires A files b.sail }"),
            ("a.sail", a),
            ("b.sail", b),
            ("shared.sail", shared),
        ],
    );
    server.did_open("shared.sail", shared);
    assert_eq!(errors(&server.pull("shared.sail")), 0, "valid upstream scattered fixture");
    let start = shared.lines().nth(1).unwrap().find("saved }").unwrap();
    let result = server.ask("experimental/ssr", serde_json::json!({
        "query":"$x ==>> $x", "textDocument":{"uri":server.url("shared.sail")},
        "position":{"line":1,"character":start},
        "selections":[{"start":{"line":1,"character":start},"end":{"line":1,"character":start+5}}]}));
    let edits = result["documentChanges"].as_array().unwrap();
    assert_eq!(edits.len(), 1, "{result}");
    assert_eq!(edits[0]["textDocument"]["version"], 1);
    assert_eq!(edits[0]["edits"].as_array().unwrap().len(), 1);
    let request = serde_json::json!({"textDocument":{"uri":server.url("shared.sail")},
        "range":{"start":{"line":1,"character":start},"end":{"line":1,"character":start+5}},
        "context":{"diagnostics":[],"only":["refactor.extract"]}});
    let actions = server.ask("textDocument/codeAction", request);
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Extract to variable")
        .expect("same extraction in both contexts")
        .clone();
    let resolved = server.ask("codeAction/resolve", action.clone());
    assert!(!resolved["edit"].is_null(), "{resolved}");
    let rename = server.ask(
        "textDocument/rename",
        serde_json::json!({
        "textDocument":{"uri":server.url("shared.sail")},
        "position":{"line":1,"character":binding}, "newName":"arg" }),
    );
    let document = &rename["documentChanges"][0];
    assert_eq!(document["textDocument"]["version"], 1, "{rename}");
    assert_eq!(
        document["edits"].as_array().unwrap().len(),
        2,
        "deduplicated local rename: {rename}"
    );
    let updated = apply_preview(shared, document);
    server.notify("textDocument/didChange", serde_json::json!({"textDocument":{"uri":server.url("shared.sail"),"version":2},"contentChanges":[{"text":updated}]}));
    let diagnostics = server.pull("shared.sail");
    assert_eq!(
        errors(&diagnostics),
        0,
        "renamed shared code must recheck: {updated}\n{diagnostics:?}"
    );
    let fresh = server.ask(
        "textDocument/rename",
        serde_json::json!({
        "textDocument":{"uri":server.url("shared.sail")},
        "position":{"line":1,"character":binding}, "newName":"argument" }),
    );
    assert_eq!(fresh["documentChanges"][0]["textDocument"]["version"], 2);
    assert_eq!(
        rename["documentChanges"][0]["textDocument"]["version"], 1,
        "the earlier edit retains the version a client must reject after didChange"
    );
    let id = server.request("codeAction/resolve", action);
    let stale =
        server.wait_for_response(id).error.expect("a stale shared-file action must be rejected");
    assert_eq!(stale.code, lsp_server::ErrorCode::ContentModified as i32);
}

#[test]
fn custom_postfix_templates_preserve_literal_receivers_on_both_client_kinds() {
    use serde_json::json;
    let source = "function f() -> string = \"cost $1 } λ\" .literal\n";
    for snippets in [false, true] {
        let mut server = ServerFixture::open_with_client(
            "custom-postfix-clients",
            &[("sail.proj", "Main { files a.sail }"), ("a.sail", source)],
            Root::Real,
            None,
            json!({"completion":{"customTemplates":[{"postfix":"literal","body":"(${receiver})$0","description":"parenthesize"}]}}),
            ClientProfile {
                completion_snippets: snippets,
                pull_diagnostics: true,
                ..Default::default()
            },
        );
        server.did_open("a.sail", source);
        let index = ide_db::line_index::LineIndex::new(source);
        let position = crate::to_proto::position_in(&index, source.trim_end().len());
        let answer = server.ask(
            "textDocument/completion",
            json!({"textDocument":{"uri":server.url("a.sail")},"position":position}),
        );
        let items = answer.as_array().or_else(|| answer["items"].as_array()).unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == ".literal")
            .unwrap_or_else(|| panic!("{answer}"));
        assert_eq!(item["insertTextFormat"], if snippets { 2 } else { 1 });
        let resolved = server.ask("completionItem/resolve", item.clone());
        assert_eq!(resolved["textEdit"], item["textEdit"]);
        let edit: lsp_types::TextEdit =
            serde_json::from_value(resolved["textEdit"].clone()).unwrap();
        let start = crate::from_proto::offset(&index, edit.range.start);
        let end = crate::from_proto::offset(&index, edit.range.end);
        assert_eq!(&source[start..end], "\"cost $1 } λ\" .literal");
        if snippets {
            assert_eq!(edit.new_text, r#"("cost \$1 \} λ")$0"#);
        } else {
            assert_eq!(edit.new_text, "(\"cost $1 } λ\")");
            let mut updated = source.to_owned();
            updated.replace_range(start..end, &edit.new_text);
            server.notify("textDocument/didChange",json!({"textDocument":{"uri":server.url("a.sail"),"version":2},"contentChanges":[{"text":updated}]}));
            assert_eq!(errors(&server.pull("a.sail")), 0);
        }
    }
}

#[test]
fn type_directed_completion_applies_and_rechecks_in_a_loaded_project() {
    use serde_json::json;
    let source = "val as_bool : int -> bool\nfunction as_bool(x) = true\nval identity : int -> int\nfunction identity(x) = x\nfunction f(value : int) -> unit = if as_ then () else ()\n";
    let mut server = ServerFixture::new_project("term-search-proof", &[("a.sail", source)]);
    server.did_open("a.sail", source);
    server.configure(json!({"completion":{"termSearch":true}}));
    let index = ide_db::line_index::LineIndex::new(source);
    let offset = source.rfind("as_").unwrap() + 3;
    let response = server.ask("textDocument/completion",json!({"textDocument":{"uri":server.url("a.sail")},"position":crate::to_proto::position_in(&index,offset)}));
    let item = response
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "as_bool(identity(value))")
        .unwrap_or_else(|| panic!("{response}"));
    let edit: lsp_types::TextEdit = serde_json::from_value(item["textEdit"].clone()).unwrap();
    let mut updated = source.to_owned();
    updated.replace_range(
        crate::from_proto::offset(&index, edit.range.start)
            ..crate::from_proto::offset(&index, edit.range.end),
        &edit.new_text,
    );
    server.notify("textDocument/didChange",json!({"textDocument":{"uri":server.url("a.sail"),"version":2},"contentChanges":[{"text":updated}]}));
    assert_eq!(errors(&server.pull("a.sail")), 0);
}

#[test]
#[ignore = "requires SAIL_EDITOR_PROTOCOL_BIN with editor diagnostic protocol v1"]
fn structured_compiler_fix_applies_rechecks_and_rejects_stale_actions() {
    use serde_json::json;
    let compiler =
        std::env::var("SAIL_EDITOR_PROTOCOL_BIN").expect("SAIL_EDITOR_PROTOCOL_BIN is required");
    assert!(std::path::Path::new(&compiler).is_absolute(), "compiler must use an absolute path");
    let source = "default Order dec\r\nlet banner = \"🙂\"\r\nlet value = 0xAb\r\n";
    let mut server = ServerFixture::open_with_client(
        "structured-compiler-live",
        &[("sail.proj", "Main { files a.sail }"), ("a.sail", source)],
        Root::Real,
        None,
        json!({"flycheck":{"enable":true,"command":compiler,"diagnosticFormat":"json","debounceMs":0}}),
        ClientProfile { pull_diagnostics: true, ..Default::default() },
    );
    server.did_open("a.sail", source);
    let uri = server.url("a.sail");
    server.wait_until("structured compiler warning", |server| {
        server.shown_for(&uri).iter().any(|d| {
            d.source.as_deref() == Some("sail")
                && d.data.as_ref().and_then(|d| d.get("sailCompiler")).is_some()
        })
    });
    let diagnostics = server.pull("a.sail");
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code
                == Some(lsp_types::NumberOrString::String("sail.warning.hex-case".into()))
        })
        .unwrap_or_else(|| panic!("{diagnostics:?}"));
    assert_eq!(
        diagnostic.range,
        lsp_types::Range::new(lsp_types::Position::new(2, 12), lsp_types::Position::new(2, 16))
    );
    let actions=server.ask("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":diagnostic.range,"context":{"diagnostics":[diagnostic],"only":["quickfix"]}}));
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Use lowercase hexadecimal digits")
        .unwrap_or_else(|| panic!("{actions}"));
    assert!(action["data"]["compilerFix"].is_object());
    let resolved = server.ask("codeAction/resolve", action.clone());
    let edits = &resolved["edit"]["documentChanges"][0];
    assert_eq!(edits["textDocument"]["version"], 1);
    let edit: lsp_types::TextEdit = serde_json::from_value(edits["edits"][0].clone()).unwrap();
    assert_eq!(edit.new_text, "0xab");
    let index = ide_db::line_index::LineIndex::new(source);
    let mut updated = source.to_owned();
    updated.replace_range(
        crate::from_proto::offset(&index, edit.range.start)
            ..crate::from_proto::offset(&index, edit.range.end),
        &edit.new_text,
    );
    server.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":updated}]}),
    );
    let _ = server.pull("a.sail");
    let id = server.request("codeAction/resolve", action.clone());
    let stale = server.wait_for_response(id);
    assert_eq!(stale.error.map(|e| e.code), Some(lsp_server::ErrorCode::ContentModified as i32));
    server.save("a.sail", &updated);
    server.wait_until("compiler warning cleared", |server| {
        !server.shown_for(&uri).iter().any(|d| d.source.as_deref() == Some("sail"))
    });
    assert_eq!(errors(&server.pull("a.sail")), 0);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let report = server.ask("sail-lsp/diagnosticReport", json!({}));
        if report["analyzerStatus"].as_str().is_some_and(|status| status.contains("Passed")) {
            break;
        }
        assert!(Instant::now() < deadline, "fresh compiler check did not pass: {report}");
        server.pump(Duration::from_millis(10));
    }
}

/// Exercise typing damage and repairs through the real event loop. Each case
/// deletes a different structural token; neighbours must retain their binding
/// and every repair must clear transient diagnostics without a restart.
#[test]
fn editing_mutations_preserve_neighbours_and_recover_after_every_repair() {
    use serde_json::json;
    let prefix = "val identity : int -> int\nfunction earlier() -> int = identity(1)\n";
    let body = "function changing(x : int) -> int = { let y : int = identity(x); y }\n";
    let suffix = "function later() -> int = identity(2)\n";
    let original = format!("{prefix}{body}{suffix}");
    let mut server = ServerFixture::new_project("editing-mutations", &[("main.sail", &original)]);
    server.did_open("main.sail", &original);
    assert_eq!(errors(&server.pull("main.sail")), 0);
    let mut version = 1;
    let mut mutations: Vec<_> = body
        .char_indices()
        .filter(|(_, c)| "():{};=".contains(*c))
        .map(|(at, character)| {
            let mut damaged = body.to_owned();
            damaged.remove(at);
            (format!("delete {character} at {at}"), damaged)
        })
        .collect();
    mutations.extend([
        ("unfinished signature".into(), "function changing(x : bits(\n".into()),
        ("unfinished call".into(), "function changing(x : int) -> int = identity(\n".into()),
        ("unfinished branch".into(), "function changing(x : int) -> int = if true then\n".into()),
        ("unfinished match".into(), "function changing(x : int) -> int = match x {\n".into()),
        ("unfinished existential".into(), "val changing : forall ('n : Int), 'n >=\n".into()),
    ]);
    for (label, damaged) in mutations {
        for (phase, source) in
            [("damaged", format!("{prefix}{damaged}{suffix}")), ("repaired", original.clone())]
        {
            version += 1;
            server.notify("textDocument/didChange", json!({"textDocument":{"uri":server.url("main.sail"),"version":version},"contentChanges":[{"text":source}]}));
            for needle in ["identity(1)", "identity(2)"] {
                let offset = source.find(needle).unwrap();
                let before = &source[..offset];
                let line = before.bytes().filter(|&byte| byte == b'\n').count();
                let column = before.rsplit('\n').next().unwrap().encode_utf16().count();
                let params = json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":line,"character":column+2}});
                let definition = server.ask("textDocument/definition", params.clone());
                let hover = server.ask("textDocument/hover", params.clone());
                let completion = server.ask("textDocument/completion", params);
                let items = completion
                    .as_array()
                    .or_else(|| completion["items"].as_array())
                    .unwrap_or_else(|| {
                        panic!("{label} {phase} {needle}: no completions: {completion}")
                    });
                assert!(
                    items.iter().any(|item| item["label"]
                        .as_str()
                        .is_some_and(|label| label.starts_with("identity"))),
                    "{label} {phase}: missing completion: {completion}"
                );
                assert!(
                    !definition.is_null()
                        && definition.as_array().is_none_or(|items| !items.is_empty()),
                    "{label} {phase}: {definition}"
                );
                assert!(!hover.is_null(), "{label} {phase}: missing hover for {needle}");
            }
            let diagnostics = server.pull("main.sail");
            assert!(
                diagnostics.len() < 15,
                "{label} {phase}: cascading diagnostics: {diagnostics:?}"
            );
            if phase == "repaired" {
                assert_eq!(errors(&diagnostics), 0, "{label}: stale errors: {diagnostics:?}");
            }
        }
    }
}

#[test]
fn preprocessing_preview_protocol_selects_visits_and_maps_current_utf16_locations() {
    use serde_json::json;
    let main = "$include \"child.sail\"\n$define SECOND\n$include \"child.sail\"\n";
    let child = "$ifdef SECOND\n/*😀*/ let second = 2\n$else\nlet first = 1\n$endif\n";
    let mut server = ServerFixture::new(
        "preview-protocol",
        &[("sail.proj", "Main { files main.sail }\n"), ("main.sail", main), ("child.sail", child)],
    );
    server.did_open("child.sail", child);
    server.pull("child.sail");
    let contexts = server.ask("sail-lsp/includeContexts", json!({"uri":server.url("child.sail")}));
    let choices = contexts.as_array().unwrap();
    assert_eq!(choices.len(), 2, "{contexts}");
    let key = choices[1]["key"].clone();
    server.ask(
        "sail-lsp/selectIncludeContext",
        json!({"textDocument":{"uri":server.url("child.sail")},"context":key}),
    );
    let result = server
        .ask("sail-lsp/expandInclude", json!({"textDocument":{"uri":server.url("child.sail")}}));
    assert_eq!(result["context"], key);
    assert_eq!(result["project"]["module"], "Main");
    let text = result["expansion"].as_str().unwrap();
    assert!(text.contains("let second") && !text.contains("let first"), "{result}");
    let mappings = result["mappings"].as_array().unwrap();
    let source = mappings
        .iter()
        .find(|mapping| {
            mapping["kind"] == "source" && mapping["origin"]["range"]["start"]["line"] == 0
        })
        .unwrap();
    assert_eq!(source["origin"]["uri"], server.url("child.sail").as_str());
    assert!(mappings
        .iter()
        .all(|mapping| mapping["range"]["end"]["line"].as_u64().unwrap()
            < text.lines().count() as u64));
    let expanded_index = ide_db::line_index::LineIndex::new(text);
    let child_index = ide_db::line_index::LineIndex::new(child);
    for mapping in mappings.iter().filter(|mapping| mapping["kind"] == "source") {
        let generated: lsp_types::Range = serde_json::from_value(mapping["range"].clone()).unwrap();
        let original: lsp_types::Range =
            serde_json::from_value(mapping["origin"]["range"].clone()).unwrap();
        assert_eq!(
            &text[crate::from_proto::offset(&expanded_index, generated.start)
                ..crate::from_proto::offset(&expanded_index, generated.end)],
            &child[crate::from_proto::offset(&child_index, original.start)
                ..crate::from_proto::offset(&child_index, original.end)]
        );
    }
    let changed = format!("// moved\n{}", child.replace("second = 2", "second = 7"));
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":server.url("child.sail"),"version":2},"contentChanges":[{"text":changed}]}));
    let changed = server
        .ask("sail-lsp/expandInclude", json!({"textDocument":{"uri":server.url("child.sail")}}));
    assert!(changed["expansion"].as_str().unwrap().contains("let second = 7"));
    assert_eq!(changed["context"], key);
}

#[test]
fn standard_source_commands_select_context_open_projects_and_show_linked_previews() {
    use serde_json::json;
    let main = "val identity : int -> int\n$include \"child.sail\"\n$define SECOND\n$include \"child.sail\"\n";
    let child =
        "$ifdef SECOND\nfunction second() -> int = identity(true)\n$else\nfunction first() -> int = 1\n$endif\n";
    let mut server = ServerFixture::new(
        "editor-entrypoints",
        &[("sail.proj", "Main { files main.sail }\n"), ("main.sail", main), ("child.sail", child)],
    );
    server.did_open("child.sail", child);
    server.pull("child.sail");
    let actions = server.ask("textDocument/codeAction", json!({"textDocument":{"uri":server.url("child.sail")},"range":{"start":{"line":1,"character":37},"end":{"line":1,"character":37}},"context":{"diagnostics":[],"only":["source.sail"],"triggerKind":1}}));
    let actions = actions.as_array().unwrap();
    let contexts: Vec<_> = actions
        .iter()
        .filter(|action| action["command"]["command"] == "sail-lsp.server.selectContext")
        .collect();
    assert_eq!(contexts.len(), 3, "{actions:?}");
    let selected = contexts.last().unwrap()["command"].clone();
    server.ask("workspace/executeCommand", selected);
    let context = server.ask("sail-lsp/includeContexts", json!({"uri":server.url("child.sail")}));
    assert!(context[1]["selected"].as_bool().unwrap(), "{context}");
    for (command, expected) in [
        ("sail-lsp.server.openProject", "Main {"),
        ("sail-lsp.server.preprocess", "function second"),
        ("sail-lsp.server.explain", "Source locations:"),
        ("sail-lsp.server.explainFormatting", "styleVersion"),
    ] {
        let action = actions.iter().find(|action| action["command"]["command"] == command).unwrap();
        let before = server.shown_documents.len();
        server.ask("workspace/executeCommand", action["command"].clone());
        server.wait_until("a standard showDocument preview", |s| s.shown_documents.len() > before);
        let shown = server.shown_documents.last().unwrap();
        let uri: Url = serde_json::from_value(shown["uri"].clone()).unwrap();
        let contents = std::fs::read_to_string(uri.to_file_path().unwrap()).unwrap();
        assert!(contents.contains(expected), "{command}: {contents}");
        if command != "sail-lsp.server.openProject" {
            assert!(uri.to_file_path().unwrap().metadata().unwrap().permissions().readonly());
            server.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":"sail-preview","version":1,"text":contents}}));
            let links =
                server.ask("textDocument/documentLink", json!({"textDocument":{"uri":uri}}));
            if command != "sail-lsp.server.explainFormatting" {
                assert!(!links.as_array().unwrap().is_empty(), "{command}: {links}");
            }
            assert!(links
                .as_array()
                .unwrap()
                .iter()
                .all(|link| link["target"].as_str().unwrap().starts_with("file:")));
        }
    }
    let quickfixes = server.ask("textDocument/codeAction", json!({"textDocument":{"uri":server.url("child.sail")},"range":{"start":{"line":1,"character":37},"end":{"line":1,"character":37}},"context":{"diagnostics":[],"only":["quickfix"]}}));
    assert!(
        quickfixes.is_null()
            || quickfixes.as_array().unwrap().iter().all(|action| action["command"].is_null())
    );
}

#[test]
fn manifest_typing_damage_keeps_loaded_neighbours_and_each_saved_repair_clears_errors() {
    use serde_json::json;
    let manifest =
        "variable MODE = enabled\nLib { files lib.sail }\nMain { requires Lib files main.sail }\n";
    let main = "function use() -> int = helper(1)\n";
    let mut server = ServerFixture::new(
        "manifest-typing-mutations",
        &[("sail.proj", manifest), ("lib.sail", "val helper : int -> int\n"), ("main.sail", main)],
    );
    server.did_open("sail.proj", manifest);
    server.did_open("main.sail", main);
    assert_eq!(errors(&server.pull("main.sail")), 0);
    let mutations = [
        "variable MODE =\nLib { files lib.sail }\nMain { requires Lib files main.sail }\n"
            .to_owned(),
        manifest.replace("Main {", "Main "),
        manifest.replace("requires Lib", "requires Missing"),
        manifest.replace("files main.sail", "files \"main.sail"),
        manifest.replace("files main.sail", "files missing.sail"),
    ];
    let mut version = 1;
    for damaged in mutations {
        for (phase, text) in [("damaged", damaged.as_str()), ("repaired", manifest)] {
            version += 1;
            server.notify("textDocument/didChange",json!({"textDocument":{"uri":server.url("sail.proj"),"version":version},"contentChanges":[{"text":text}]}));
            server.save("sail.proj", text);
            server.notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes":[{"uri":server.url("sail.proj"),"type":2}]}),
            );
            server.wait_for_diagnostics("sail.proj", phase, |diagnostics| {
                if phase == "repaired" {
                    diagnostics.is_empty()
                } else {
                    errors(diagnostics) > 0
                }
            });
            let definition = server.ask("textDocument/definition",json!({"textDocument":{"uri":server.url("main.sail")},"position":{"line":0,"character":26}}));
            if phase == "damaged" && text.contains("missing.sail") {
                let symbols = server.ask("workspace/symbol", json!({"query":"helper"}));
                assert!(
                    symbols.to_string().contains("lib.sail"),
                    "unaffected module lost: {symbols}"
                );
            } else {
                assert!(
                    definition.to_string().contains("lib.sail"),
                    "{phase}: {text}\n{definition}"
                );
            }
            if phase == "repaired" {
                assert_eq!(errors(&server.pull("main.sail")), 0);
                let graph = server.ask("sail-lsp/projectGraph", serde_json::Value::Null);
                assert_eq!(graph[0]["modules"].as_array().unwrap().len(), 2, "{graph}");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn standard_task_commands_revalidate_execute_arguments_and_cancel_processes() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    let mut server =
        ServerFixture::new_project("standard-task-commands", &[("main.sail", "val x : int\n")]);
    let program = server.root.join("task adapter");
    std::fs::write(&program, "#!/bin/sh\nprintf '%s\\n' \"$1\" \"$2\" \"$SAMPLE\"\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    server.configure(json!({"flycheck":{"enable":false},"runnables":{"tasks":[{"label":"context test","kind":"test","command":program,"args":["${projectContext}","literal $(never-execute) && text"],"environment":{"SAMPLE":"spaces and quotes ' value"}}]}}));
    server.did_open("main.sail", "val x : int\n");
    server.pull("main.sail");
    let tasks = server
        .ask("experimental/runnables", json!({"textDocument":{"uri":server.url("main.sail")}}));
    let task = tasks.as_array().unwrap().iter().find(|task| task["kind"] == "shell").unwrap();
    let command = json!({"command":"sail-lsp.server.runTask","arguments":[{"textDocument":{"uri":server.url("main.sail")},"id":task["id"],"fingerprint":task["fingerprint"]}]});
    server.ask("workspace/executeCommand", command.clone());
    server.wait_until("task output", |s| !s.shown_documents.is_empty());
    let uri: Url =
        serde_json::from_value(server.shown_documents.last().unwrap()["uri"].clone()).unwrap();
    let output = std::fs::read_to_string(uri.to_file_path().unwrap()).unwrap();
    assert!(
        output.contains("Exit: exit status: 0")
            && output.contains("literal $(never-execute) && text")
            && output.contains("spaces and quotes ' value"),
        "{output}"
    );
    assert!(output.contains("\"manifest\"") && output.contains("\"modules\""), "{output}");
    server.configure(json!({"flycheck":{"enable":false},"runnables":{"tasks":[]}}));
    let id = server.request("workspace/executeCommand", command);
    let result = server.wait_for_response(id);
    assert!(result.error.is_some(), "a stale task ran: {result:?}");
    let pid_file = server.root.join("task-pid");
    std::fs::write(&program, format!("#!/bin/sh\necho $$ > '{}'\nsleep 30\n", pid_file.display()))
        .unwrap();
    server.configure(json!({"flycheck":{"enable":false},"runnables":{"tasks":[{"label":"cancel test","kind":"test","command":program,"args":["${projectContext}"]}]}}));
    let tasks = server
        .ask("experimental/runnables", json!({"textDocument":{"uri":server.url("main.sail")}}));
    let task = tasks.as_array().unwrap().iter().find(|task| task["kind"] == "shell").unwrap();
    let before = server.shown_documents.len();
    server.ask("workspace/executeCommand",json!({"command":"sail-lsp.server.runTask","arguments":[{"textDocument":{"uri":server.url("main.sail")},"id":task["id"],"fingerprint":task["fingerprint"]}]}));
    server.wait_until("task starts", |_| pid_file.exists());
    let pid = std::fs::read_to_string(pid_file).unwrap().trim().parse::<i32>().unwrap();
    server.ask(
        "workspace/executeCommand",
        json!({"command":"sail-lsp.server.cancelTask","arguments":[]}),
    );
    server.wait_until("task cancelled", |s| s.shown_documents.len() > before);
    // SAFETY: signal 0 checks the PID written by the fixture's own process.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "cancelled task was not reaped");
    let uri: Url =
        serde_json::from_value(server.shown_documents.last().unwrap()["uri"].clone()).unwrap();
    assert!(std::fs::read_to_string(uri.to_file_path().unwrap()).unwrap().contains("Cancelled"));
}

#[test]
fn formatting_invalid_style_is_reported_without_hiding_unrelated_quick_fixes() {
    use serde_json::json;
    let source = "function f() = { let unused=1; 2 }\n";
    let mut server =
        ServerFixture::new_project("formatting-invalid-style", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let diagnostics = server.pull("main.sail");
    std::fs::write(server.root.join(".sailfmt.toml"), "line_wdth=50\n").unwrap();
    let params = json!({"textDocument":{"uri":server.url("main.sail")},"options":{"tabSize":2,"insertSpaces":true}});
    let id = server.request("textDocument/formatting", params);
    let response = server.wait_for_response(id);
    let error = response.error.unwrap().message;
    assert!(error.contains(".sailfmt.toml") && error.contains("line_wdth"), "{error}");
    let actions=server.ask("textDocument/codeAction",json!({"textDocument":{"uri":server.url("main.sail")},"range":{"start":{"line":0,"character":21},"end":{"line":0,"character":27}},"context":{"diagnostics":diagnostics}}));
    let actions = actions.as_array().unwrap();
    assert!(
        actions.iter().any(|a| a["title"].as_str().is_some_and(|t| t.contains("unused"))),
        "{actions:?}"
    );
    assert!(!actions.iter().any(|a| a["title"] == "Format document"));
}

#[test]
fn formatting_on_type_obeys_each_documents_options_and_project_style() {
    use serde_json::json;
    let source = "function f() = {\n    \n}\n";
    let mut server =
        ServerFixture::new_project("formatting-typing", &[("a.sail", source), ("b.sail", source)]);
    server.did_open("a.sail", source);
    server.did_open("b.sail", source);
    for (file, tab_size, spaces, expected) in
        [("a.sail", 2, true, "  "), ("b.sail", 4, false, "\t"), ("a.sail", 4, true, "    ")]
    {
        let params = json!({"textDocument":{"uri":server.url(file)},"position":{"line":1,"character":4},"ch":"\n","options":{"tabSize":tab_size,"insertSpaces":spaces}});
        let edits = server.ask("textDocument/onTypeFormatting", params);
        assert_eq!(edits[0]["newText"], expected, "{edits}");
    }
    let edits = server.ask(
        "experimental/onEnter",
        json!({"textDocument":{"uri":server.url("b.sail")},"position":{"line":1,"character":4}}),
    );
    assert_eq!(edits[0]["newText"], "\t", "options from another document leaked: {edits}");
    std::fs::write(server.root.join(".sailfmt.toml"), "tab_size=8\ninsert_spaces=true\n").unwrap();
    let edits=server.ask("textDocument/onTypeFormatting",json!({"textDocument":{"uri":server.url("a.sail")},"position":{"line":1,"character":4},"ch":"\n","options":{"tabSize":2,"insertSpaces":true}}));
    assert_eq!(edits[0]["newText"], "        ");
    let info = server
        .ask("sail-lsp/explainFormatting", json!({"textDocument":{"uri":server.url("a.sail")}}));
    assert_eq!(info["options"]["tabSize"], 8);
    assert!(info["configFile"].as_str().unwrap().ends_with(".sailfmt.toml"));
    assert_eq!(info["status"], "changed");
    std::fs::write(server.root.join(".sailfmt.toml"), "").unwrap();
    let edits = server.ask(
        "experimental/onEnter",
        json!({"textDocument":{"uri":server.url("a.sail")},"position":{"line":1,"character":4}}),
    );
    assert_eq!(edits[0]["newText"], "  ", "removed project override remained cached: {edits}");
    server.notify("textDocument/didChange",json!({"textDocument":{"uri":server.url("a.sail"),"version":2},"contentChanges":[{"text":"function f() = {\n"}]}));
    let info = server
        .ask("sail-lsp/explainFormatting", json!({"textDocument":{"uri":server.url("a.sail")}}));
    assert_eq!(info["status"], "parse_error");
}

#[test]
fn formatting_returns_small_utf16_edits_and_formats_selected_body_lines() {
    use serde_json::json;
    let source="// 😀 untouched\nfunction f(x:int) -> int = {\n  /*😀*/ let y=x+1;\n  y+2\n}\nfunction unchanged() = 7\n";
    let mut server = ServerFixture::new_project("formatting-local-edits", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    let params = json!({"textDocument":{"uri":server.url("main.sail")},"options":{"tabSize":2,"insertSpaces":true}});
    let edits = server.ask("textDocument/formatting", params.clone());
    assert!(edits.as_array().unwrap().len() > 1, "{edits}");
    assert!(edits.as_array().unwrap().iter().all(|edit| edit["range"]["start"]["line"]
        .as_u64()
        .unwrap()
        > 0
        && edit["range"]["end"]["line"].as_u64().unwrap() < 5));
    let mut params = params;
    params["range"] = json!({"start":{"line":2,"character":0},"end":{"line":3,"character":0}});
    let edits = server.ask("textDocument/rangeFormatting", params);
    let after = apply_preview(source, &json!({"edits":edits}));
    assert!(after.starts_with("// 😀 untouched\nfunction f(x:int) -> int = {\n"));
    assert!(after.contains("  /*😀*/ let y = x + 1;\n  y+2"), "{after}");
    let explanation=server.ask("sail-lsp/explainFormatting",json!({"textDocument":{"uri":server.url("main.sail")},"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}}}));
    assert_eq!(explanation["status"], "range_boundary", "{explanation}");
}

#[test]
fn inlay_settings_reach_chain_types_and_generated_enum_values_over_the_wire() {
    use serde_json::json;
    let source = "enum Color = { Red, Green }\nstruct Address = { zip : int }\nstruct Person = { address : Address }\nfunction f(p : Person) -> int = p.address.zip\n";
    let mut server = ServerFixture::new_project("inlay-feature-settings", &[("main.sail", source)]);
    server.did_open("main.sail", source);
    for enabled in [true, false] {
        server.configure(json!({"inlayHints":{"enumValueHints":enabled,"chainingHints":enabled}}));
        let hints = server.ask("textDocument/inlayHint",json!({"textDocument":{"uri":server.url("main.sail")},"range":{"start":{"line":0,"character":0},"end":{"line":4,"character":0}}}));
        let hints = hints.as_array().unwrap();
        let enum_values: Vec<_> =
            hints.iter().filter(|hint| hint["data"]["kind"] == "enumValue").collect();
        let chaining: Vec<_> =
            hints.iter().filter(|hint| hint["data"]["kind"] == "chaining").collect();
        assert_eq!(enum_values.len(), if enabled { 2 } else { 0 }, "{hints:?}");
        assert_eq!(chaining.len(), usize::from(enabled), "{hints:?}");
        if enabled {
            assert_eq!(enum_values[1]["label"], "= 1");
            assert_eq!(inlay_label(chaining[0]), ": Address");
        }
    }
}

/// An assist leaves the cursor where the user types next — for a client
/// that said it can take a snippet in a workspace edit. One that did not
/// gets the same stub as plain text, with nothing left to expand.
#[test]
fn an_assist_places_the_cursor_only_for_a_client_that_takes_snippets() {
    use serde_json::json;
    let source = "val f : int -> int\nfunction f(x) = helper(x)\n";
    for snippets in [false, true] {
        let mut server = ServerFixture::open_with_client(
            "assist-snippet-clients",
            &[("sail.proj", "Main { files a.sail }"), ("a.sail", source)],
            Root::Real,
            Some(false),
            serde_json::Value::Null,
            ClientProfile {
                experimental: json!({ "snippetTextEdit": snippets, "serverStatusNotification": true }),
                ..Default::default()
            },
        );
        server.did_open("a.sail", source);
        let at = source.find("helper").unwrap() - source.find("function").unwrap();
        let actions = server.ask("textDocument/codeAction", json!({
            "textDocument": {"uri": server.url("a.sail")},
            "range": {"start": {"line": 1, "character": at}, "end": {"line": 1, "character": at}},
            "context": {"diagnostics": []},
        }));
        let action = actions
            .as_array()
            .and_then(|actions| {
                actions.iter().find(|action| action["title"] == "Generate function `helper`")
            })
            .unwrap_or_else(|| panic!("{actions}"));
        assert!(action["edit"].is_null(), "the edit is resolved lazily: {action}");
        let resolved = server.ask("codeAction/resolve", action.clone());
        let edit: crate::lsp_ext::SnippetWorkspaceEdit =
            serde_json::from_value(resolved["edit"].clone()).unwrap();
        let edits = &edit.changes.unwrap()[&server.url("a.sail")];
        assert_eq!(edits.len(), 1, "{edits:?}");
        if snippets {
            assert_eq!(edits[0].insert_text_format, Some(lsp_types::InsertTextFormat::SNIPPET));
            assert_eq!(
                edits[0].new_text,
                "\n\nval helper : _ -> ${1:_}\nfunction helper(arg1) = $0\n"
            );
        } else {
            assert_eq!(edits[0].insert_text_format, Some(lsp_types::InsertTextFormat::PLAIN_TEXT));
            assert_eq!(edits[0].new_text, "\n\nval helper : _ -> _\nfunction helper(arg1) = ()\n");
        }
    }
}

/// `sail-lsp/serverStatus` goes to a client that declared
/// `experimental.serverStatusNotification`, and to no other.
#[test]
fn server_status_is_sent_only_to_a_client_that_asked_for_it() {
    for declared in [true, false] {
        let mut server = ServerFixture::open_with_client(
            "server-status-capability",
            &[("sail.proj", "Main { files main.sail }"), ("main.sail", A_FILE_WITH_ONE_ERROR)],
            Root::Real,
            None,
            serde_json::Value::Null,
            ClientProfile {
                experimental: if declared {
                    serde_json::json!({ "serverStatusNotification": true })
                } else {
                    serde_json::json!({})
                },
                ..Default::default()
            },
        );
        server.did_open("main.sail", A_FILE_WITH_ONE_ERROR);
        server.wait_for_diagnostics("main.sail", "the error", |d| errors(d) == 1);
        // Loading and the diagnostics wave each move the status; a client
        // that asked has heard about it by now.
        assert_eq!(!server.statuses.is_empty(), declared, "{:?}", server.statuses);
    }
}

/// A reference lens runs the client's `sail-lsp.showReferences`, with the
/// protocol arguments, when the client listed that command under
/// `experimental.commands`; a client that did not gets the count alone.
#[test]
fn lens_commands_are_the_clients_or_nothing() {
    use serde_json::json;
    let source = "val helper : int -> int\nfunction helper(value) = value\nfunction main() -> int = helper(1)\n";
    for listed in [true, false] {
        let commands: Vec<&str> = if listed { vec!["sail-lsp.showReferences"] } else { vec![] };
        let mut server = ServerFixture::open_with_client(
            "lens-client-commands",
            &[("sail.proj", "Main { files main.sail }"), ("main.sail", source)],
            Root::Real,
            None,
            serde_json::Value::Null,
            ClientProfile {
                experimental: json!({ "commands": { "commands": commands } }),
                ..Default::default()
            },
        );
        server.did_open("main.sail", source);
        let lenses = server.ask(
            "textDocument/codeLens",
            json!({"textDocument": {"uri": server.url("main.sail")}}),
        );
        let lens = lenses
            .as_array()
            .unwrap()
            .iter()
            .find(|lens| lens["data"]["kind"] == "refs" && lens["range"]["start"]["line"] == 1)
            .unwrap_or_else(|| panic!("{lenses}"));
        let resolved = server.ask("codeLens/resolve", lens.clone());
        assert_eq!(resolved["command"]["title"], "1 reference", "{resolved}");
        if listed {
            assert_eq!(resolved["command"]["command"], "sail-lsp.showReferences", "{resolved}");
            let arguments = resolved["command"]["arguments"].as_array().unwrap();
            assert_eq!(arguments[0], json!(server.url("main.sail")));
            assert_eq!(arguments[1], lens["range"]["start"]);
            assert_eq!(arguments[2].as_array().unwrap().len(), 1, "{resolved}");
        } else {
            assert_eq!(resolved["command"]["command"], "", "{resolved}");
            assert!(resolved["command"]["arguments"].is_null(), "{resolved}");
        }
    }
}

/// A client that resolves tooltips lazily gets a parameter hint without one
/// and `inlayHint/resolve` fills it in; a client that cannot gets the hint
/// complete. The server advertises `resolveProvider` to match.
#[test]
fn inlay_tooltips_are_resolved_lazily_only_for_a_client_that_asks() {
    use serde_json::json;
    let source = "val helper : int -> int\nfunction helper(value) = value\nfunction main() -> int = helper(1)\n";
    for lazy in [true, false] {
        let mut server = ServerFixture::open_with_client(
            "inlay-resolve-clients",
            &[("sail.proj", "Main { files main.sail }"), ("main.sail", source)],
            Root::Real,
            None,
            serde_json::Value::Null,
            ClientProfile { inlay_hint_resolve: lazy, ..Default::default() },
        );
        assert_eq!(server.capabilities["inlayHintProvider"]["resolveProvider"], json!(lazy));
        server.did_open("main.sail", source);
        let hints = server.ask(
            "textDocument/inlayHint",
            json!({
                "textDocument": {"uri": server.url("main.sail")},
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 0}},
            }),
        );
        let parameter = hints
            .as_array()
            .unwrap()
            .iter()
            .find(|hint| inlay_label(hint) == "value:")
            .unwrap_or_else(|| panic!("{hints}"));
        let tooltip = json!("Parameter hint for `value`");
        if lazy {
            assert!(parameter["tooltip"].is_null(), "{parameter}");
            let resolved = server.ask("inlayHint/resolve", parameter.clone());
            assert_eq!(resolved["tooltip"], tooltip, "{resolved}");
            assert_eq!(resolved["label"], parameter["label"]);
        } else {
            assert_eq!(parameter["tooltip"], tooltip, "{parameter}");
        }
    }
}
