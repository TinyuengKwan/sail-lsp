//! Standard LSP entrypoints for clients without a custom Sail UI. Code actions
//! contain concrete choices; execution revalidates them against current state.
use crate::{global_state::GlobalState, lsp_ext, main_loop::Task};
use ide_db::SourceFileInfo as _;
use lsp_types::{
    CodeActionParams, Command, DocumentLink, ExecuteCommandParams, Location, MessageType, Url,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

pub(crate) const COMMANDS: &[&str] = &[
    "sail-lsp.server.selectContext",
    "sail-lsp.server.openProject",
    "sail-lsp.server.explain",
    "sail-lsp.server.preprocess",
    "sail-lsp.server.runTask",
    "sail-lsp.server.cancelTask",
    "sail-lsp.server.explainFormatting",
];

#[derive(Debug)]
pub(crate) struct Document {
    pub text: String,
    pub links: Vec<DocumentLink>,
}

#[derive(Debug)]
pub(crate) struct SourceStamp {
    uri: Url,
    document: Option<(i32, u64)>,
    nonce: u64,
    context: Option<String>,
}

#[derive(Default)]
pub(crate) struct State {
    directory: Option<tempfile::TempDir>,
    paths: std::collections::VecDeque<std::path::PathBuf>,
    pub links: Arc<HashMap<Url, Vec<DocumentLink>>>,
    sequence: u64,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

fn writable(path: &std::path::Path) {
    if let Ok(metadata) = path.metadata() {
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(path, permissions);
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for path in &self.paths {
            writable(path);
        }
    }
}
impl State {
    fn store(&mut self, document: Document) -> anyhow::Result<Url> {
        if self.directory.is_none() {
            self.directory = Some(tempfile::tempdir()?);
        }
        self.sequence += 1;
        let path = self
            .directory
            .as_ref()
            .unwrap()
            .path()
            .join(format!("analysis-{}.sail-preview", self.sequence));
        std::fs::write(&path, document.text)?;
        let mut permissions = path.metadata()?.permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions)?;
        let uri =
            Url::from_file_path(&path).map_err(|_| anyhow::anyhow!("Invalid preview path"))?;
        Arc::make_mut(&mut self.links).insert(uri.clone(), document.links);
        self.paths.push_back(path);
        while self.paths.len() > 16 {
            let old = self.paths.pop_front().unwrap();
            if let Ok(uri) = Url::from_file_path(&old) {
                Arc::make_mut(&mut self.links).remove(&uri);
            }
            writable(&old);
            let _ = std::fs::remove_file(old);
        }
        Ok(uri)
    }
}

fn action(title: String, command: &str, arguments: Value) -> lsp_ext::CodeAction {
    lsp_ext::CodeAction {
        title: title.clone(),
        kind: Some(lsp_types::CodeActionKind::new("source.sail")),
        command: Some(Command { title, command: command.into(), arguments: Some(vec![arguments]) }),
        ..Default::default()
    }
}

pub(crate) fn actions(
    snap: &crate::global_state::GlobalStateSnapshot,
    params: &CodeActionParams,
) -> anyhow::Result<Vec<lsp_ext::CodeAction>> {
    if params.context.trigger_kind == Some(lsp_types::CodeActionTriggerKind::AUTOMATIC)
        || !crate::code_action_helpers::code_action_kind_allowed(
            &params.context.only,
            &lsp_types::CodeActionKind::new("source.sail"),
        )
    {
        return Ok(vec![]);
    }
    let uri = &params.text_document.uri;
    let position = params.range.start;
    let document = json!({"textDocument":{"uri":uri},"position":position,"sourceFingerprint":snap.analysis.file(uri).map(|file|crate::code_action_helpers::source_fingerprint(file.text()))});
    let mut out = Vec::new();
    if snap.client_caps.show_document_support() {
        for (title, command) in [
            ("Open project manifest", COMMANDS[1]),
            ("Explain analysis at cursor", COMMANDS[2]),
            ("Explain formatting", COMMANDS[6]),
            ("Preview preprocessed source", COMMANDS[3]),
        ] {
            out.push(action(format!("Sail: {title}"), command, document.clone()));
        }
    }
    let contexts = snap.analysis.include_contexts(uri)?;
    if contexts.len() > 1 {
        out.push(action(
            "Sail: Use automatic project/include context".into(),
            COMMANDS[0],
            json!({"textDocument":{"uri":uri},"context":null}),
        ));
        for (_, key, root) in contexts {
            let module = key.module.and_then(|index| snap.module_contexts.get(index));
            let label = format!(
                "Sail: Context {} / {} / include {:?}",
                module.and_then(|m| m["manifest"].as_str()).unwrap_or(root.as_str()),
                module.and_then(|m| m["module"].as_str()).unwrap_or("global"),
                key.includes
            );
            out.push(action(label, COMMANDS[0], json!({"textDocument":{"uri":uri},"context":crate::handlers::request::include_context_key(&key,&root,&snap.module_contexts)})));
        }
    }
    for task in crate::runnables::discover(snap, uri)? {
        if task.disabled.is_some() {
            continue;
        }
        out.push(action(
            format!("Sail: Run {}", task.label),
            COMMANDS[4],
            json!({"textDocument":{"uri":uri},"id":task.id,"fingerprint":task.fingerprint}),
        ));
    }
    out.push(action(
        "Sail: Cancel project task".into(),
        COMMANDS[5],
        json!({"textDocument":{"uri":uri}}),
    ));
    Ok(out)
}

pub(crate) fn execute(
    state: &mut GlobalState,
    params: ExecuteCommandParams,
) -> anyhow::Result<Option<Value>> {
    anyhow::ensure!(
        COMMANDS.contains(&params.command.as_str()),
        "Unknown Sail command: {}",
        params.command
    );
    if params.command == COMMANDS[5] {
        state.editor_commands.cancel.store(true, Ordering::Release);
        let scope = params
            .arguments
            .first()
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        crate::handlers::notification::handle_cancel_flycheck(state, scope)?;
        return Ok(None);
    }
    let input = params.arguments.first().cloned().ok_or_else(|| {
        anyhow::anyhow!("Use a Sail source action to supply the document and command arguments")
    })?;
    let document: lsp_types::TextDocumentIdentifier =
        serde_json::from_value(input["textDocument"].clone())?;
    if params.command == COMMANDS[0] {
        crate::handlers::request::handle_select_include_context(
            state,
            serde_json::from_value(input)?,
        )?;
        return Ok(None);
    }
    let mut snapshot = state.snapshot();
    snapshot.config = state.config_for_uri(&document.uri).clone();
    if params.command == COMMANDS[2]
        && input["sourceFingerprint"].as_str().is_some_and(|expected| {
            snapshot.analysis.file(&document.uri).is_none_or(|file| {
                crate::code_action_helpers::source_fingerprint(file.text()) != expected
            })
        })
    {
        return Err(crate::dispatch::RequestError {
            code: lsp_server::ErrorCode::ContentModified,
            message: "The source changed; request its analysis action again".into(),
        }
        .into());
    }
    if params.command == COMMANDS[4] {
        let id = input["id"].as_str().ok_or_else(|| anyhow::anyhow!("Missing runnable id"))?;
        let runnable = crate::runnables::discover(&snapshot, &document.uri)?
            .into_iter()
            .find(|task| task.id == id)
            .ok_or_else(|| anyhow::anyhow!("Task no longer exists"))?;
        anyhow::ensure!(
            runnable.disabled.is_none() && input["fingerprint"] == runnable.fingerprint,
            "Task changed; request source actions again"
        );
        anyhow::ensure!(
            !runnable.documents.iter().any(|uri| state.dirty_documents.contains(uri)),
            "Save project documents before running this task"
        );
        match runnable.command {
            lsp_ext::RunnableCommand::Check(scope) => {
                crate::handlers::notification::handle_run_flycheck(state, scope)?
            }
            lsp_ext::RunnableCommand::Process(process) => {
                anyhow::ensure!(
                    state.editor_commands.worker.as_ref().is_none_or(JoinHandle::is_finished),
                    "A Sail task is running; cancel it before starting another"
                );
                if let Some(previous) = state.editor_commands.worker.take() {
                    let _ = previous.join();
                }
                let cancel = Arc::new(AtomicBool::new(false));
                state.editor_commands.cancel = cancel.clone();
                let sender = state.task_pool.sender();
                let timeout =
                    std::time::Duration::from_millis(snapshot.config.runnables.timeout_ms.max(1));
                let label = runnable.label;
                state.editor_commands.worker =
                    Some(std::thread::Builder::new().name("sail-editor-task".into()).spawn(
                        move || {
                            let text = crate::flycheck::run_task(&process, timeout, &cancel);
                            let _ = sender.send(Task::EditorDocument {
                                source: None,
                                result: Ok(Document {
                                    text: format!("{label}\n{text}"),
                                    links: vec![],
                                }),
                            });
                        },
                    )?);
            }
        }
        return Ok(None);
    }
    if params.command == COMMANDS[1] {
        let location = crate::handlers::request::handle_open_project_manifest(
            snapshot,
            lsp_ext::ProjectNavigationParams { text_document: document },
        )?;
        if let Some(location) = location {
            show(state, location.uri, Some(location.range));
        }
        return Ok(None);
    }
    anyhow::ensure!(state.client_caps.show_document_support(), "This client does not support window/showDocument; use sail-lsp/explainAnalysis or sail-lsp/expandInclude directly");
    let source = SourceStamp {
        uri: document.uri.clone(),
        document: state
            .mem_docs
            .get(&document.uri)
            .map(|document| (document.version, document.lifetime)),
        nonce: state.analysis_host.raw_database().nonce(),
        context: state
            .mem_docs
            .get(&document.uri)
            .and_then(|document| document.selected_context.clone()),
    };
    let command = params.command;
    state.task_pool.spawn(stdx::thread::ThreadIntent::Worker, move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> anyhow::Result<Document> {
                if command == COMMANDS[6] {
                    let info = crate::formatting::explain(
                        snapshot,
                        lsp_ext::ExplainFormattingParams {
                            text_document: document,
                            options: None,
                            range: None,
                        },
                    )?;
                    Ok(Document { text: serde_json::to_string_pretty(&info)?, links: vec![] })
                } else if command == COMMANDS[3] {
                    let result = crate::handlers::request::handle_expand_include(
                        snapshot,
                        lsp_ext::ExpandIncludeParams { text_document: document },
                    )?
                    .ok_or_else(|| {
                        anyhow::anyhow!("No preprocessing visit in the loaded project")
                    })?;
                    Ok(Document {
                        text: result.expansion,
                        links: result
                            .mappings
                            .into_iter()
                            .map(|mapping| DocumentLink {
                                range: mapping.range,
                                target: Some(location_uri(&mapping.origin)),
                                tooltip: Some("Open original source".into()),
                                data: None,
                            })
                            .collect(),
                    })
                } else {
                    let position = serde_json::from_value(input["position"].clone())?;
                    let result = crate::handlers::request::handle_explain_analysis(
                        snapshot,
                        lsp_types::TextDocumentPositionParams { text_document: document, position },
                    )?;
                    explanation_document(result)
                }
            },
        ))
        .unwrap_or_else(|error| {
            if error.is::<salsa::Cancelled>() {
                Err(anyhow::anyhow!("Source changed; request the explanation again"))
            } else {
                Err(anyhow::anyhow!("Internal error while preparing Sail analysis"))
            }
        })
        .map_err(|error| error.to_string());
        Task::EditorDocument { source: Some(source), result }
    });
    Ok(Some(json!({"queued":true})))
}

fn location_uri(location: &Location) -> Url {
    let mut uri = location.uri.clone();
    uri.set_fragment(Some(&format!(
        "L{},{}",
        location.range.start.line + 1,
        location.range.start.character + 1
    )));
    uri
}

fn explanation_document(result: Value) -> anyhow::Result<Document> {
    let value = |value: &Value| value.as_str().unwrap_or("(unknown)").to_owned();
    let mut text = format!("Analysis at {}\n", value(&result["file"]));
    if let Some(error) = result["projectLoadError"].as_str() {
        text.push_str(&format!("Project load error: {error}\n"));
    }
    if let Some(contexts) = result["contexts"].as_array() {
        for context in contexts {
            text.push_str(&format!(
                "\nSource region: {}\nResolution: {}\n",
                if context["active"] == true { "active" } else { "inactive" },
                value(&context["resolution"])
            ));
            if context["project"].is_object() {
                text.push_str(&format!(
                    "Project: {}\nModule: {}\n",
                    value(&context["project"]["manifest"]),
                    value(&context["project"]["module"])
                ));
            }
            let types = &context["types"];
            if let Some(inferred) = types["inferred"].as_str() {
                text.push_str(&format!("Inferred type: {inferred}\n"));
            }
            for mismatch in types["mismatches"].as_array().into_iter().flatten() {
                text.push_str(&format!(
                    "Expected: {}\nActual: {}\n",
                    value(&mismatch["expected"]),
                    value(&mismatch["actual"])
                ));
                for origin in mismatch["origins"].as_array().into_iter().flatten() {
                    text.push_str(&format!("  {}\n", value(&origin["label"])));
                }
            }
            for candidate in types["candidates"].as_array().into_iter().flatten() {
                text.push_str(&format!(
                    "{} candidate: {}\n  Signature: {}\n  Reason: {}\n",
                    if candidate["status"] == "unknown" { "Unproven" } else { "Rejected" },
                    value(&candidate["target"]),
                    value(&candidate["signature"]),
                    value(&candidate["reason"])
                ));
                for index in candidate["argumentIndices"].as_array().into_iter().flatten() {
                    text.push_str(&format!("  Related explicit argument: {index}\n"));
                }
            }
            for constraint in types["constraints"].as_array().into_iter().flatten() {
                text.push_str(&format!(
                    "Constraint ({}): {}\n  Reason: {}\n",
                    if constraint["status"] == "refuted" {
                        "proved false"
                    } else {
                        "not established"
                    },
                    value(&constraint["constraint"]),
                    value(&constraint["reason"])
                ));
                for assumption in constraint["assumptions"].as_array().into_iter().flatten() {
                    text.push_str(&format!("  Assuming: {}\n", value(assumption)));
                }
            }
        }
    }
    text.push_str("\n\nSource locations:\n");
    let mut locations = Vec::new();
    fn collect(value: &Value, locations: &mut Vec<Location>) {
        if let Ok(location) = serde_json::from_value::<Location>(value.clone()) {
            if location.uri.scheme() == "file" && !locations.contains(&location) {
                locations.push(location);
            }
            return;
        }
        match value {
            Value::Array(items) => {
                for value in items {
                    collect(value, locations)
                }
            }
            Value::Object(items) => {
                for value in items.values() {
                    collect(value, locations)
                }
            }
            _ => {}
        }
    }
    collect(&result, &mut locations);
    let mut links = Vec::new();
    for location in locations {
        let start = text.len();
        text.push_str(location_uri(&location).as_str());
        let index = ide_db::line_index::LineIndex::new(&text);
        links.push(DocumentLink {
            range: crate::to_proto::range(&index, base_db::text_range(start, text.len())),
            target: Some(location_uri(&location)),
            tooltip: Some("Open declaration/argument".into()),
            data: None,
        });
        text.push('\n');
    }
    Ok(Document { text, links })
}

fn show(state: &mut GlobalState, uri: Url, selection: Option<lsp_types::Range>) {
    state.send_request::<lsp_types::request::ShowDocument>(
        lsp_types::ShowDocumentParams {
            uri,
            external: Some(false),
            take_focus: Some(true),
            selection,
        },
        |state, response| {
            if response.error.is_some()
                || response.result.as_ref().is_none_or(|result| result["success"] != true)
            {
                state.log_message(MessageType::WARNING, "Client could not open the Sail document");
            }
        },
    );
}

pub(crate) fn present(
    state: &mut GlobalState,
    source: Option<SourceStamp>,
    result: Result<Document, String>,
) {
    if source.as_ref().is_some_and(|stamp| {
        let current = state.mem_docs.get(&stamp.uri);
        current.map(|document| (document.version, document.lifetime)) != stamp.document
            || current.and_then(|document| document.selected_context.clone()) != stamp.context
            || state.analysis_host.raw_database().nonce() != stamp.nonce
    }) {
        state.log_message(
            MessageType::INFO,
            "Sail source changed while preparing its preview; request again",
        );
        return;
    }
    let stored = result
        .map_err(anyhow::Error::msg)
        .and_then(|document| state.editor_commands.store(document));
    match stored {
        Ok(uri) => {
            if state.client_caps.show_document_support() {
                show(state, uri, None);
            } else {
                state.log_message(MessageType::INFO, format!("Sail task output: {uri}"));
            }
        }
        Err(error) => state.log_message(MessageType::ERROR, format!("Sail command: {error}")),
    }
}

#[cfg(test)]
mod source_stamp_tests {
    use super::*;

    #[test]
    fn completed_preview_is_presented_only_for_its_original_document() {
        for edited in [false, true] {
            let (wire, messages) = crossbeam_channel::unbounded();
            let (tasks, completed) = crossbeam_channel::unbounded();
            let mut state = GlobalState::new(wire, tasks);
            state.client_caps.0 =
                serde_json::from_value(json!({"window":{"showDocument":{"support":true}}}))
                    .unwrap();
            let directory = tempfile::tempdir().unwrap();
            let uri = Url::from_file_path(directory.path().join("main.sail")).unwrap();
            crate::handlers::notification::handle_did_open_text_document(
                &mut state,
                serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"sail","version":1,"text":"function f() -> int = 1\n"}})).unwrap(),
            ).unwrap();
            state.process_changes();
            execute(&mut state, serde_json::from_value(json!({"command":COMMANDS[2],"arguments":[{"textDocument":{"uri":uri},"position":{"line":0,"character":10}}]})).unwrap()).unwrap();
            let Task::EditorDocument { source, result } =
                completed.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
            else {
                panic!("expected completed preview");
            };
            assert!(result.is_ok(), "{result:?}");
            if edited {
                crate::handlers::notification::handle_did_change_text_document(
                    &mut state,
                    serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"function f() -> int = 2\n"}]})).unwrap(),
                ).unwrap();
                state.process_changes();
            }
            present(&mut state, source, result);
            let shown = messages.try_iter().filter(|message| matches!(message,
                lsp_server::Message::Request(request) if request.method == "window/showDocument"
            )).count();
            assert_eq!(
                shown,
                usize::from(!edited),
                "a completed old preview must not be presented after a source edit"
            );
            assert_eq!(state.editor_commands.links.len(), usize::from(!edited));
        }
    }
}
