use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::{Path};
use std::sync::{OnceLock, Arc};
use std::time::Duration;

use lsp_server::{Connection, Message, Response as ServerResponse, Notification as ServerNotification};
use lsp_types::{
    InitializeParams, ServerCapabilities, TextDocumentSyncKind, Url, OneOf, CompletionOptions,
    HoverProviderCapability, Diagnostic, DiagnosticSeverity, PublishDiagnosticsParams, Range, Position
};
use lsp_types::request::Request;
use lsp_types::notification::Notification;
use clap::Parser;

mod utils;
mod repl;
mod state;
mod handlers;

use crate::state::{SailState, get_diag_regex};
use crate::utils::apply_changes;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the Sail executable
    #[arg(long, default_value = "sail")]
    pub sail_path: String,
}

pub static ARGS: OnceLock<Args> = OnceLock::new();

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args = Args::parse();
    ARGS.set(args).unwrap();

    env_logger::init();
    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(&ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncKind::INCREMENTAL.into()),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["$".to_string(), "#".to_string(), ".".to_string(), ":".to_string(), " ".to_string()]),
            ..Default::default()
        }),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Left(true)),
        ..Default::default()
    })?;

    let initialization_params = connection.initialize(server_capabilities)?;
    let init_params: InitializeParams = serde_json::from_value(initialization_params)?;

    let (diag_tx, diag_rx) = crossbeam_channel::unbounded::<(Url, bool)>();
    let mut state = SailState::new(diag_tx);
    if let Some(ref root) = init_params.root_uri {
        if let Ok(path) = root.to_file_path() { state.project_root = Some(path); }
    }
    state.index_project();

    let state = Arc::new(state);
    let sender = connection.sender.clone();
    
    // Debounce thread for diagnostics
    let state_for_diag = Arc::clone(&state);
    std::thread::spawn(move || {
        let mut pending_requests = HashMap::new();
        loop {
            match diag_rx.recv_timeout(Duration::from_millis(500)) {
                Ok((uri, force)) => {
                    let entry = pending_requests.entry(uri).or_insert(false);
                    *entry |= force;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if !pending_requests.is_empty() {
                        let requests: Vec<(Url, bool)> = pending_requests.drain().collect();
                        let any_force = requests.iter().any(|(_, f)| *f);
                        let first_uri = requests[0].0.clone();
                        
                        if let Ok(path) = first_uri.to_file_path() {
                            let output = {
                                let mut repl = state_for_diag.repl.lock().unwrap();
                                if !any_force && repl.is_alive() {
                                    repl.send_command(":reload")
                                } else {
                                    let sail_path = &ARGS.get().unwrap().sail_path;
                                    let file_to_load = state_for_diag.find_sail_root(&path).unwrap_or(path);
                                    repl.spawn(sail_path, &file_to_load).unwrap_or_default()
                                }
                            };
                            state_for_diag.index_project();
                            
                            let uris_to_clear: HashSet<Url> = requests.into_iter().map(|(u, _)| u).collect();
                            publish_diagnostics_batch(&sender, &first_uri, &state_for_diag, output, uris_to_clear);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    main_loop(connection, state)?;
    io_threads.join()?;
    Ok(())
}

fn main_loop(connection: Connection, state: Arc<SailState>) -> Result<(), Box<dyn Error + Send + Sync>> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? { return Ok(()); }
                let state = Arc::clone(&state);
                let sender = connection.sender.clone();
                std::thread::spawn(move || {
                    let id = req.id.clone();
                    let result = match req.method.as_str() {
                        lsp_types::request::HoverRequest::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_hover(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::GotoDefinition::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_definition(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::Completion::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_completion(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::DocumentSymbolRequest::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_document_symbols(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::WorkspaceSymbolRequest::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_workspace_symbols(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::Formatting::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_formatting(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::References::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_references(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        lsp_types::request::Rename::METHOD => {
                            serde_json::from_value(req.params).ok().and_then(|p| {
                                serde_json::to_value(handlers::handle_rename(&state, p)).ok()
                            }).unwrap_or(serde_json::Value::Null)
                        }
                        _ => serde_json::Value::Null,
                    };
                    let _ = sender.send(Message::Response(ServerResponse { id, result: Some(result), error: None }));
                });
            }
            Message::Notification(not) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    match not.method.as_str() {
                        lsp_types::notification::DidOpenTextDocument::METHOD => {
                            if let Ok(params) = serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(not.params) {
                                state.files.write().unwrap().insert(params.text_document.uri.clone(), params.text_document.text);
                                let _ = state.diag_tx.send((params.text_document.uri, true));
                            }
                        }
                        lsp_types::notification::DidChangeTextDocument::METHOD => {
                            if let Ok(params) = serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(not.params) {
                                {
                                    let mut files = state.files.write().unwrap();
                                    if let Some(content) = files.get_mut(&params.text_document.uri) {
                                        apply_changes(content, params.content_changes);
                                    }
                                }
                                let _ = state.diag_tx.send((params.text_document.uri, false));
                            }
                        }
                        lsp_types::notification::DidSaveTextDocument::METHOD => {
                            if let Ok(params) = serde_json::from_value::<lsp_types::DidSaveTextDocumentParams>(not.params) {
                                let _ = state.diag_tx.send((params.text_document.uri, false));
                            }
                        }
                        lsp_types::notification::DidCloseTextDocument::METHOD => {
                            if let Ok(params) = serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(not.params) {
                                state.files.write().unwrap().remove(&params.text_document.uri);
                            }
                        }
                        _ => {}
                    }
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn publish_diagnostics_batch(sender: &crossbeam_channel::Sender<Message>, uri: &Url, state: &SailState, output: Vec<String>, mut uris_to_report: HashSet<Url>) {
    let mut file_diagnostics: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    let re_loc = get_diag_regex();
    
    let mut current_diag: Option<(Url, Diagnostic)> = None;
    for line in output {
        if let Some(caps) = re_loc.captures(&line) {
            if let Some((u, d)) = current_diag.take() {
                file_diagnostics.entry(u).or_default().push(d);
            }
            
            let Some(file_path_str) = caps.get(1).map(|m| m.as_str()) else { continue };
            let path = Path::new(file_path_str);
            let absolute_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                state.project_root.as_ref().cloned().unwrap_or_else(|| {
                    uri.to_file_path().unwrap_or_default().parent().unwrap_or(Path::new("")).to_path_buf()
                }).join(path)
            };
            
            if let Ok(target_uri) = Url::from_file_path(absolute_path) {
                let l1: u32 = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(1).saturating_sub(1);
                let c1: u32 = caps.get(3).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
                let l2: u32 = caps.get(4).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(1).saturating_sub(1);
                let c2: u32 = caps.get(5).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
                let message = caps.get(6).map(|m| m.as_str().to_string()).unwrap_or_default();
                
                uris_to_report.insert(target_uri.clone());
                current_diag = Some((target_uri, Diagnostic {
                    range: Range {
                        start: Position { line: l1, character: c1 },
                        end: Position { line: l2, character: c2 },
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    message,
                    ..Default::default()
                }));
            }
        } else if line.starts_with("STDERR:") {
            if let Some((_, ref mut d)) = current_diag {
                d.message.push('\n');
                d.message.push_str(&line[7..]);
            }
        }
    }
    if let Some((u, d)) = current_diag {
        file_diagnostics.entry(u).or_default().push(d);
    }

    for u in uris_to_report {
        let diagnostics = file_diagnostics.remove(&u).unwrap_or_default();
        let _ = sender.send(Message::Notification(ServerNotification {
            method: "textDocument/publishDiagnostics".to_string(),
            params: serde_json::to_value(PublishDiagnosticsParams {
                uri: u,
                diagnostics,
                version: None,
            }).unwrap(),
        }));
    }
}
