use state::{scan_folders, File, Files};
use std::collections::hash_map::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::request::{
    GotoDeclarationParams, GotoDeclarationResponse, GotoImplementationParams,
    GotoImplementationResponse, GotoTypeDefinitionParams, GotoTypeDefinitionResponse,
};
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CallHierarchyServerCapability, CodeAction, CodeActionKind, CodeActionOptions,
    CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
    CodeLens, CodeLensOptions, CodeLensParams, Command, CompletionItem, CompletionOptions,
    CompletionParams, CompletionResponse, DeclarationCapability, DiagnosticOptions,
    DiagnosticServerCapabilities, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
    DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReportResult,
    DocumentFormattingParams, DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
    DocumentLink, DocumentLinkOptions, DocumentLinkParams, DocumentOnTypeFormattingOptions,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, ExecuteCommandParams, FileOperationFilter, FileOperationPattern,
    FileOperationPatternKind, FileOperationRegistrationOptions, FileSystemWatcher, FoldingRange,
    FoldingRangeKind, FoldingRangeParams, GlobPattern, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    ImplementationProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    InlayHint, InlayHintOptions, InlayHintParams, InlayHintServerCapabilities,
    LinkedEditingRangeParams, LinkedEditingRangeServerCapabilities, LinkedEditingRanges, Location,
    MessageType, OneOf, PrepareRenameResponse, Range, ReferenceParams, Registration,
    RenameFilesParams, RenameParams, SelectionRange, SelectionRangeParams,
    SelectionRangeProviderCapability, SemanticTokens, SemanticTokensDeltaParams,
    SemanticTokensFullDeltaResult, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, ServerCapabilities, SignatureHelp,
    SignatureHelpOptions, SignatureHelpParams, SymbolInformation, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, TypeDefinitionProviderCapability,
    TypeHierarchyItem, TypeHierarchyOptions, TypeHierarchyPrepareParams,
    TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, Url, WatchKind,
    WorkDoneProgressOptions, WorkspaceDiagnosticParams, WorkspaceDiagnosticReportResult,
    WorkspaceEdit, WorkspaceFileOperationsServerCapabilities, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities, WorkspaceSymbol, WorkspaceSymbolOptions, WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod actions;
mod completion;
mod diagnostics;
mod formatting;
mod hover;
mod inlay_hints;
mod semantic_tokens;
mod state;
mod symbols;

#[cfg(test)]
use actions::missing_semicolon_fix;
use actions::{
    code_action_kind_allowed, default_code_action_format_options, lazy_code_action_data,
    resolve_code_action_edit_from_data, sail_source_fix_all_kind,
};
use actions::{extract_local_let_edits, organize_imports_edits, quick_fix_for_diagnostic};
use completion::{
    build_completion_items, completion_prefix, completion_trigger_characters,
    resolve_completion_item,
};
use diagnostics::{document_diagnostic_report_for_file, workspace_diagnostic_report};
use formatting::{
    document_links_for_file, format_document_edits, linked_editing_ranges_for_position,
    make_selection_range, range_format_document_edits,
};
#[cfg(test)]
use formatting::{format_document_text, range_len};
use hover::hover_for_symbol;
use inlay_hints::{inlay_hints_for_range, resolve_inlay_hint};
use semantic_tokens::{
    compute_semantic_tokens, compute_semantic_tokens_delta, compute_semantic_tokens_range,
    semantic_tokens_options,
};
#[cfg(test)]
use symbols::analysis::Parameter;
use symbols::find_call_at_position;
use symbols::normalize_validated_rename;
use symbols::signature_help_for_position;
use symbols::{
    call_edges, call_hierarchy_item, implementation_locations, parse_named_type,
    resolve_workspace_symbol, symbol_declaration_locations, symbol_definition_locations,
    type_definition_locations, type_hierarchy_item, type_name_candidates_at_position,
    type_subtypes, type_supertypes, typed_bindings, will_rename_file_edits,
};
#[cfg(test)]
use symbols::{call_edges_for_file, type_alias_edges};
use symbols::{
    code_lens_title, code_lenses_for_file, collect_implementation_counts, collect_reference_counts,
};
#[cfg(test)]
use symbols::{collect_callable_signatures, function_snippet, infer_binding_type};
use symbols::{
    extract_symbol_decls, find_callable_signature, token_is_close_bracket, token_is_open_bracket,
    token_symbol_key,
};
use symbols::{reference_locations, rename_edits, resolve_symbol_at, symbol_spans_for_file};
#[cfg(test)]
use tower_lsp::lsp_types::{
    DocumentDiagnosticReport, FormattingOptions, SymbolKind, WorkspaceLocation,
};

#[derive(Default)]
struct State {
    disk_files: Files,
    open_files: HashMap<Url, File>,
    diagnostic_versions: HashMap<Url, i32>,
    semantic_tokens_cache: HashMap<Url, SemanticTokens>,
}

impl State {
    /// Get all the files, ignoring files on disk that are also open.
    fn all_files(&self) -> impl Iterator<Item = (&Url, &File)> {
        self.open_files.iter().chain(
            self.disk_files
                .all_files()
                .filter(|(uri, _)| !self.open_files.contains_key(uri)),
        )
    }
}

struct Backend {
    state: Arc<Mutex<State>>,
    client: Client,
}

const SAIL_KEYWORDS: &[&str] = &[
    "and",
    "as",
    "assert",
    "backwards",
    "bitfield",
    "by",
    "catch",
    "clause",
    "configuration",
    "constant",
    "constraint",
    "dec",
    "default",
    "do",
    "else",
    "end",
    "enum",
    "forall",
    "foreach",
    "forwards",
    "function",
    "if",
    "in",
    "inc",
    "infix",
    "infixl",
    "infixr",
    "instantiation",
    "let",
    "mapping",
    "match",
    "newtype",
    "overload",
    "pure",
    "ref",
    "register",
    "repeat",
    "return",
    "scattered",
    "sizeof",
    "struct",
    "termination_measure",
    "then",
    "throw",
    "try",
    "type",
    "union",
    "until",
    "val",
    "var",
    "while",
    "with",
];

const SAIL_BUILTINS: &[&str] = &[
    "Int",
    "Nat",
    "Type",
    "Order",
    "Bool",
    "true",
    "false",
    "undefined",
    "bitzero",
    "bitone",
];

const DIAGNOSTIC_DEBOUNCE_MS: u64 = 250;

impl Backend {
    pub fn new_with_client(client: Client) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            client,
        }
    }

    fn schedule_debounced_diagnostics(&self, uri: Url, version: i32) {
        let state = self.state.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DIAGNOSTIC_DEBOUNCE_MS)).await;
            let diagnostics = {
                let state_guard = state.lock().await;
                if state_guard.diagnostic_versions.get(&uri).copied() != Some(version) {
                    return;
                }
                state_guard
                    .open_files
                    .get(&uri)
                    .map(|file| file.lsp_diagnostics())
            };
            if let Some(diagnostics) = diagnostics {
                client.publish_diagnostics(uri, diagnostics, None).await;
            }
        });
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.client
            .log_message(MessageType::INFO, "server initialized")
            .await;

        let mut state = self.state.lock().await;
        if let Some(workspace_folders) = params.workspace_folders {
            for folder in workspace_folders {
                state.disk_files.add_folder(folder.uri);
            }
        }

        let folders = state.disk_files.folders().clone();
        state.disk_files.update(scan_folders(folders));

        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                declaration_provider: Some(DeclarationCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Right(WorkspaceSymbolOptions {
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(false),
                    },
                    resolve_provider: Some(true),
                })),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::SOURCE,
                            CodeActionKind::SOURCE_FIX_ALL,
                            sail_source_fix_all_kind(),
                            CodeActionKind::REFACTOR_REWRITE,
                            CodeActionKind::REFACTOR_EXTRACT,
                            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                        ]),
                        resolve_provider: Some(true),
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: Some(false),
                        },
                    },
                )),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(true),
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                    first_trigger_character: "}".to_string(),
                    more_trigger_character: Some(vec![";".to_string()]),
                }),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(false),
                    },
                }),
                document_highlight_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: Some(false),
                        },
                        resolve_provider: Some(true),
                    },
                ))),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                folding_range_provider: Some(
                    tower_lsp::lsp_types::FoldingRangeProviderCapability::Simple(true),
                ),
                semantic_tokens_provider: Some(semantic_tokens_options().into()),
                linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(
                    true,
                )),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("sail".to_string()),
                        inter_file_dependencies: true,
                        workspace_diagnostics: true,
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: Some(false),
                        },
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                execute_command_provider: Some(tower_lsp::lsp_types::ExecuteCommandOptions {
                    commands: vec!["sail.noop".to_string(), "sail.run".to_string()],
                    ..Default::default()
                }),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(completion_trigger_characters()),
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(false),
                    },
                    all_commit_characters: None,
                    completion_item: None,
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(false),
                    },
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        did_create: None,
                        will_create: None,
                        did_rename: None,
                        will_rename: Some(FileOperationRegistrationOptions {
                            filters: vec![
                                FileOperationFilter {
                                    scheme: Some("file".to_string()),
                                    pattern: FileOperationPattern {
                                        glob: "**/*.sail".to_string(),
                                        matches: Some(FileOperationPatternKind::File),
                                        options: None,
                                    },
                                },
                                FileOperationFilter {
                                    scheme: Some("file".to_string()),
                                    pattern: FileOperationPattern {
                                        glob: "**".to_string(),
                                        matches: Some(FileOperationPatternKind::Folder),
                                        options: None,
                                    },
                                },
                            ],
                        }),
                        did_delete: None,
                        will_delete: None,
                    }),
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "server initialized")
            .await;

        // Technically we should check if the client capabilities support this
        // but I can't be bothered.

        // TODO: This is kind of broken. If you rename a folder that contains some of
        // these files then you won't get a notification for them. The easiest
        // solution is to watch all files.

        let result = self
            .client
            .register_capability(vec![
                Registration {
                    id: "sail_watch_files_id".to_string(),
                    method: "workspace/didChangeWatchedFiles".to_string(),
                    register_options: Some(
                        serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                            watchers: vec![FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.sail".to_string()),
                                kind: Some(WatchKind::all()),
                            }],
                        })
                        .unwrap(),
                    ),
                },
                Registration {
                    id: "sail_type_hierarchy_id".to_string(),
                    method: "textDocument/prepareTypeHierarchy".to_string(),
                    register_options: Some(
                        serde_json::to_value(TypeHierarchyOptions {
                            work_done_progress_options: WorkDoneProgressOptions {
                                work_done_progress: Some(false),
                            },
                        })
                        .unwrap(),
                    ),
                },
            ])
            .await;

        match result {
            Ok(()) => {
                self.client
                    .log_message(
                        MessageType::INFO,
                        "registered file watcher and type hierarchy",
                    )
                    .await;
            }
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("error registering file watcher: {:?}", e),
                    )
                    .await;
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        self.client
            .log_message(MessageType::INFO, "workspace folders changed")
            .await;

        let mut state = self.state.lock().await;

        for folder in params.event.added.iter() {
            state.disk_files.add_folder(folder.uri.clone());
        }
        for folder in params.event.removed.iter() {
            state.disk_files.remove_folder(&folder.uri);
        }
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        self.client
            .log_message(MessageType::INFO, "configuration changed")
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut files = String::new();
        for change in &params.changes {
            files.push_str(&format!(" {}", change.uri));
        }
        self.client
            .log_message(
                MessageType::INFO,
                format!("watched files have changed: {}", files),
            )
            .await;

        let mut state = self.state.lock().await;
        for change in &params.changes {
            match change.typ {
                tower_lsp::lsp_types::FileChangeType::DELETED => {
                    state.disk_files.remove_file(&change.uri);
                }
                tower_lsp::lsp_types::FileChangeType::CREATED
                | tower_lsp::lsp_types::FileChangeType::CHANGED => {
                    // Parse the file.
                    if change.uri.scheme() == "file" {
                        if let Ok(path) = change.uri.to_file_path() {
                            if let Ok(source) = std::fs::read_to_string(path) {
                                let file = File::new(source);
                                state.disk_files.add_file(change.uri.clone(), file);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!("file opened: {}", params.text_document.uri),
            )
            .await;

        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let file = File::new(params.text_document.text /*, true*/);
        {
            let mut state = self.state.lock().await;
            state.diagnostic_versions.insert(uri.clone(), version);
            state.open_files.insert(uri.clone(), file);
        }
        self.schedule_debounced_diagnostics(uri, version);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!("file changed: {}", params.text_document.uri),
            )
            .await;

        let uri = &params.text_document.uri;
        let version = params.text_document.version;

        let mut state = self.state.lock().await;

        let file = state
            .open_files
            .get_mut(uri)
            .expect("document changed that isn't open");
        file.update(params.content_changes);
        state.diagnostic_versions.insert(uri.clone(), version);
        self.schedule_debounced_diagnostics(uri.clone(), version);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!("file saved: {}", params.text_document.uri),
            )
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!("file closed: {}", params.text_document.uri),
            )
            .await;
        let uri = &params.text_document.uri;

        let mut state = self.state.lock().await;
        state.open_files.remove(uri);
        state.diagnostic_versions.remove(uri);
        state.semantic_tokens_cache.remove(uri);
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.client
            .log_message(MessageType::INFO, format!("goto definition: {:?}", params))
            .await;

        let uri = &params.text_document_position_params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };

        let position = params.text_document_position_params.position;

        if let Some(token) = file.token_at(position) {
            if let Some(symbol_key) = token_symbol_key(&token.0) {
                if symbol_key.starts_with('\'') {
                    return Ok(None);
                }
                let definitions = symbol_definition_locations(state.all_files(), uri, &symbol_key);

                if !definitions.is_empty() {
                    return Ok(Some(GotoDefinitionResponse::Array(definitions)));
                }
            }
        }
        Ok(None)
    }

    async fn goto_declaration(
        &self,
        params: GotoDeclarationParams,
    ) -> Result<Option<GotoDeclarationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, _)) = file.token_at(position) else {
            return Ok(None);
        };
        let Some(symbol_key) = token_symbol_key(token) else {
            return Ok(None);
        };
        if symbol_key.starts_with('\'') {
            return Ok(None);
        }

        let declarations = symbol_declaration_locations(state.all_files(), uri, &symbol_key);
        if !declarations.is_empty() {
            return Ok(Some(GotoDeclarationResponse::Array(declarations)));
        }
        let declarations = symbol_definition_locations(state.all_files(), uri, &symbol_key);
        if declarations.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoDeclarationResponse::Array(declarations)))
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };

        let all_files = state.all_files().collect::<Vec<_>>();
        for name in type_name_candidates_at_position(file, position) {
            if let Some(item) = type_hierarchy_item(all_files.iter().copied(), uri, &name) {
                return Ok(Some(vec![item]));
            }
        }
        Ok(None)
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let name = params
            .item
            .data
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&params.item.name)
            .to_string();
        let uri = &params.item.uri;

        let state = self.state.lock().await;
        let items = type_supertypes(state.all_files(), uri, &name);
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(items))
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let name = params
            .item
            .data
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&params.item.name)
            .to_string();
        let uri = &params.item.uri;

        let state = self.state.lock().await;
        let items = type_subtypes(state.all_files(), uri, &name);
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(items))
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, _)) = file.token_at(position) else {
            return Ok(None);
        };

        let mut type_names = Vec::<String>::new();
        if let Some(name) = token_symbol_key(token) {
            if !name.starts_with('\'') {
                type_names.push(name.clone());
                if let Some(ty) = typed_bindings(file).get(&name).cloned() {
                    type_names.push(ty);
                }
                if let Some((callee, _)) = find_call_at_position(file, position) {
                    if callee == name {
                        if let Some(ret) = find_callable_signature(state.all_files(), uri, &callee)
                            .and_then(|sig| sig.return_type)
                            .and_then(|rt| parse_named_type(&rt))
                        {
                            type_names.push(ret);
                        }
                    }
                }
            }
        }

        type_names.sort();
        type_names.dedup();

        let mut locations = Vec::<Location>::new();
        for ty in type_names {
            locations.extend(type_definition_locations(state.all_files(), uri, &ty));
        }
        if locations.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoTypeDefinitionResponse::Array(locations)))
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, _)) = file.token_at(position) else {
            return Ok(None);
        };
        let Some(name) = token_symbol_key(token) else {
            return Ok(None);
        };
        if name.starts_with('\'') {
            return Ok(None);
        }

        let locations = implementation_locations(state.all_files(), uri, &name);
        if locations.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoImplementationResponse::Array(locations)))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };

        let Some(symbol) = resolve_symbol_at(file, position) else {
            return Ok(None);
        };

        let highlights = symbol_spans_for_file(file, &symbol, true)
            .into_iter()
            .map(|(span, is_write)| DocumentHighlight {
                range: Range::new(
                    file.source.position_at(span.start),
                    file.source.position_at(span.end),
                ),
                kind: Some(if is_write {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                }),
            })
            .collect::<Vec<_>>();

        Ok(Some(highlights))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };

        let include_declaration = params.context.include_declaration;
        let Some(symbol) = resolve_symbol_at(file, position) else {
            return Ok(None);
        };
        let locations = reference_locations(state.all_files(), uri, &symbol, include_declaration);

        Ok(Some(locations))
    }

    #[allow(deprecated)]
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };

        let symbols = extract_symbol_decls(file)
            .into_iter()
            .map(|decl| {
                let range = Range::new(
                    file.source.position_at(decl.offset),
                    file.source.position_at(decl.offset + decl.name.len()),
                );
                SymbolInformation {
                    name: decl.name,
                    kind: decl.kind,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri.clone(), range),
                    container_name: Some(decl.detail.to_string()),
                }
            })
            .collect::<Vec<_>>();

        Ok(Some(DocumentSymbolResponse::Flat(symbols)))
    }

    #[allow(deprecated)]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.to_ascii_lowercase();
        let state = self.state.lock().await;
        let mut symbols = Vec::new();

        for (uri, file) in state.all_files() {
            for decl in extract_symbol_decls(file) {
                if !query.is_empty() && !decl.name.to_ascii_lowercase().contains(&query) {
                    continue;
                }
                let range = Range::new(
                    file.source.position_at(decl.offset),
                    file.source.position_at(decl.offset + decl.name.len()),
                );
                symbols.push(SymbolInformation {
                    name: decl.name,
                    kind: decl.kind,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri.clone(), range),
                    container_name: Some(decl.detail.to_string()),
                });
            }
        }

        Ok(Some(symbols))
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        let state = self.state.lock().await;
        Ok(resolve_workspace_symbol(params, state.all_files()))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, _)) = file.token_at(position) else {
            return Ok(None);
        };
        let Some(name) = token_symbol_key(token) else {
            return Ok(None);
        };
        if name.starts_with('\'') {
            return Ok(None);
        }
        let Some(item) = call_hierarchy_item(state.all_files(), uri, &name) else {
            return Ok(None);
        };
        Ok(Some(vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let target_name = params
            .item
            .data
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&params.item.name)
            .to_string();

        let state = self.state.lock().await;
        let edges = call_edges(state.all_files());
        let mut grouped: HashMap<String, (Url, Vec<Range>)> = HashMap::new();
        for edge in edges {
            if edge.callee != target_name {
                continue;
            }
            let entry = grouped
                .entry(edge.caller.clone())
                .or_insert_with(|| (edge.caller_uri.clone(), Vec::new()));
            entry.1.push(edge.call_range);
        }

        let mut calls = Vec::new();
        for (caller, (uri, from_ranges)) in grouped {
            if let Some(from_item) = call_hierarchy_item(state.all_files(), &uri, &caller) {
                calls.push(CallHierarchyIncomingCall {
                    from: from_item,
                    from_ranges,
                });
            }
        }
        Ok(Some(calls))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let caller_name = params
            .item
            .data
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&params.item.name)
            .to_string();
        let caller_uri = params.item.uri.clone();

        let state = self.state.lock().await;
        let edges = call_edges(state.all_files());
        let mut grouped: HashMap<String, Vec<Range>> = HashMap::new();
        for edge in edges {
            if edge.caller != caller_name {
                continue;
            }
            grouped
                .entry(edge.callee.clone())
                .or_default()
                .push(edge.call_range);
        }

        let mut calls = Vec::new();
        for (callee, from_ranges) in grouped {
            if let Some(to_item) = call_hierarchy_item(state.all_files(), &caller_uri, &callee) {
                calls.push(CallHierarchyOutgoingCall {
                    to: to_item,
                    from_ranges,
                });
            }
        }
        Ok(Some(calls))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let Some(tokens) = file.tokens.as_ref() else {
            return Ok(None);
        };

        let mut stack: Vec<(sail_parser::Token, u32)> = Vec::new();
        let mut ranges = Vec::new();

        for (token, span) in tokens {
            if token_is_open_bracket(token) {
                let start = file.source.position_at(span.start).line;
                stack.push((token.clone(), start));
                continue;
            }
            if !token_is_close_bracket(token) {
                continue;
            }
            let end = file.source.position_at(span.end).line;
            let Some((_open, start)) = stack.pop() else {
                continue;
            };
            if end <= start {
                continue;
            }
            ranges.push(FoldingRange {
                start_line: start,
                start_character: None,
                end_line: end,
                end_character: None,
                kind: Some(FoldingRangeKind::Region),
                collapsed_text: None,
            });
        }

        Ok(Some(ranges))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };

        let ranges = params
            .positions
            .iter()
            .map(|pos| make_selection_range(file, *pos))
            .collect::<Vec<_>>();
        Ok(Some(ranges))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };

        let requested_kinds = params.context.only.clone();
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        let mut fix_all_edits: Vec<TextEdit> = Vec::new();
        let source_fix_all_kind = sail_source_fix_all_kind();

        for diagnostic in &params.context.diagnostics {
            if let Some((title, edit, is_preferred)) = quick_fix_for_diagnostic(file, diagnostic) {
                let kind = CodeActionKind::QUICKFIX;
                fix_all_edits.push(edit.clone());
                if code_action_kind_allowed(&requested_kinds, &kind) {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title,
                        kind: Some(kind),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: None,
                        command: None,
                        is_preferred: Some(is_preferred),
                        disabled: None,
                        data: Some(lazy_code_action_data(uri, &[edit])),
                    }));
                }
            }
        }

        let format_options = default_code_action_format_options();

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::SOURCE) {
            if let Some(edits) = format_document_edits(file, &format_options) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Format document".to_string(),
                    kind: Some(CodeActionKind::SOURCE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
        }

        if fix_all_edits.len() > 1
            && code_action_kind_allowed(&requested_kinds, &CodeActionKind::SOURCE_FIX_ALL)
        {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fix all obvious parse errors".to_string(),
                kind: Some(CodeActionKind::SOURCE_FIX_ALL),
                diagnostics: None,
                edit: None,
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: Some(lazy_code_action_data(uri, &fix_all_edits)),
            }));
        }

        if code_action_kind_allowed(&requested_kinds, &source_fix_all_kind) {
            let mut aggregate_edits = fix_all_edits.clone();
            if let Some(mut edits) = organize_imports_edits(file) {
                aggregate_edits.append(&mut edits);
            }
            if !aggregate_edits.is_empty() {
                let mut deduped: Vec<TextEdit> = Vec::with_capacity(aggregate_edits.len());
                for edit in aggregate_edits {
                    if !deduped
                        .iter()
                        .any(|e| e.range == edit.range && e.new_text == edit.new_text)
                    {
                        deduped.push(edit);
                    }
                }
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Apply Sail source fixes".to_string(),
                    kind: Some(source_fix_all_kind),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &deduped)),
                }));
            }
        }

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::REFACTOR_REWRITE) {
            if let Some(edits) = range_format_document_edits(file, params.range, &format_options) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Normalize formatting in selection".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = format_document_edits(file, &format_options) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Normalize formatting in file".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
        }

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::REFACTOR_EXTRACT) {
            if let Some(edits) = extract_local_let_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Extract selection to local `let`".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_EXTRACT),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
        }

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS) {
            if let Some(edits) = organize_imports_edits(file) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Organize $include directives".to_string(),
                    kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    async fn code_action_resolve(&self, mut params: CodeAction) -> Result<CodeAction> {
        if params.edit.is_none() {
            if let Some(data) = params.data.as_ref() {
                if let Some((uri, edits)) = resolve_code_action_edit_from_data(data) {
                    let mut changes = HashMap::new();
                    changes.insert(uri, edits);
                    params.edit = Some(WorkspaceEdit {
                        changes: Some(changes),
                        document_changes: None,
                        change_annotations: None,
                    });
                }
            }
        }
        Ok(params)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };

        let offset = file.source.offset_at(&position);
        let prefix = completion_prefix(file.source.text(), offset);
        let all_files = state.all_files().collect::<Vec<_>>();
        let mut items = build_completion_items(
            all_files.iter().copied(),
            uri,
            file.source.text(),
            offset,
            prefix,
            SAIL_KEYWORDS,
            SAIL_BUILTINS,
        );

        if items.is_empty() {
            return Ok(None);
        }

        Ok(Some(CompletionResponse::Array(std::mem::take(&mut items))))
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        let state = self.state.lock().await;
        let all_files = state.all_files().collect::<Vec<_>>();
        item = resolve_completion_item(item, all_files.iter().copied());
        Ok(item)
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(tower_lsp::lsp_types::DocumentDiagnosticReport::Full(
                tower_lsp::lsp_types::RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report:
                        tower_lsp::lsp_types::FullDocumentDiagnosticReport {
                            result_id: Some("missing".to_string()),
                            items: Vec::new(),
                        },
                },
            )
            .into());
        };

        Ok(document_diagnostic_report_for_file(
            file,
            params.previous_result_id.as_deref(),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let state = self.state.lock().await;
        let previous = params
            .previous_result_ids
            .into_iter()
            .map(|entry| (entry.uri, entry.value))
            .collect::<HashMap<_, _>>();
        Ok(workspace_diagnostic_report(
            state.all_files(),
            &state.diagnostic_versions,
            &previous,
        ))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, span)) = file.token_at(position) else {
            return Ok(None);
        };
        let Some(symbol_key) = token_symbol_key(token) else {
            return Ok(None);
        };

        Ok(hover_for_symbol(
            state.all_files(),
            uri,
            file,
            position,
            Range::new(
                file.source.position_at(span.start),
                file.source.position_at(span.end),
            ),
            &symbol_key,
        ))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;

        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(signature_help_for_position(
            state.all_files(),
            uri,
            file,
            position,
        ))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let hints = inlay_hints_for_range(state.all_files(), uri, file, params.range);

        if hints.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hints))
        }
    }

    async fn inlay_hint_resolve(&self, params: InlayHint) -> Result<InlayHint> {
        Ok(resolve_inlay_hint(params))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let mut state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let tokens = compute_semantic_tokens(file);
        state
            .semantic_tokens_cache
            .insert(uri.clone(), tokens.clone());

        Ok(Some(tokens.into()))
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
        let uri = &params.text_document.uri;
        let mut state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let current = compute_semantic_tokens(file);

        let result = match state.semantic_tokens_cache.get(uri) {
            Some(previous)
                if previous.result_id.as_deref() == Some(params.previous_result_id.as_str()) =>
            {
                SemanticTokensFullDeltaResult::TokensDelta(compute_semantic_tokens_delta(
                    previous, &current,
                ))
            }
            _ => SemanticTokensFullDeltaResult::Tokens(current.clone()),
        };
        state.semantic_tokens_cache.insert(uri.clone(), current);

        Ok(Some(result))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(Some(
            compute_semantic_tokens_range(file, &params.range).into(),
        ))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(format_document_edits(file, &params.options))
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        if params.ch != "}" && params.ch != ";" {
            return Ok(None);
        }

        let uri = &params.text_document_position.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(format_document_edits(file, &params.options))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(range_format_document_edits(
            file,
            params.range,
            &params.options,
        ))
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        Ok(linked_editing_ranges_for_position(file, position))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let links = document_links_for_file(uri, file);
        if links.is_empty() {
            return Ok(None);
        }
        Ok(Some(links))
    }

    async fn document_link_resolve(&self, mut params: DocumentLink) -> Result<DocumentLink> {
        if params.target.is_none() {
            if let Some(target_str) = params
                .data
                .as_ref()
                .and_then(|v| v.get("target"))
                .and_then(|v| v.as_str())
            {
                params.target = Url::parse(target_str).ok();
            }
        }
        Ok(params)
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let state = self.state.lock().await;
        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));
        let Some(file) = file else {
            return Ok(None);
        };
        let all_files = state.all_files().collect::<Vec<_>>();
        let ref_counts = collect_reference_counts(&all_files);
        let impl_counts = collect_implementation_counts(&all_files);
        let lenses = code_lenses_for_file(file, &ref_counts, &impl_counts);
        if lenses.is_empty() {
            return Ok(None);
        }
        Ok(Some(lenses))
    }

    async fn code_lens_resolve(&self, mut params: CodeLens) -> Result<CodeLens> {
        if params.command.is_none() {
            if let Some(data) = params.data.as_ref() {
                if let Some(title) = code_lens_title(data) {
                    params.command = Some(Command {
                        title,
                        command: "sail.noop".to_string(),
                        arguments: None,
                    });
                }
            }
        }
        Ok(params)
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            "sail.run" => {
                let name = params
                    .arguments
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                self.client
                    .log_message(MessageType::INFO, format!("Running Sail function: {name}"))
                    .await;
                // In a full implementation, this could spawn a process or trigger a task
            }
            _ => {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!("Executed command: {}", params.command),
                    )
                    .await;
            }
        }
        Ok(None)
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        let state = self.state.lock().await;
        let Some(changes) = will_rename_file_edits(state.all_files(), &params) else {
            return Ok(None);
        };
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let state = self.state.lock().await;

        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };

        let Some((token, _)) = file.token_at(position) else {
            return Ok(None);
        };
        let Some(validated_name) =
            normalize_validated_rename(token, &params.new_name, SAIL_KEYWORDS)?
        else {
            return Ok(None);
        };
        let Some(symbol) = resolve_symbol_at(file, position) else {
            return Ok(None);
        };
        let changes = rename_edits(state.all_files(), uri, &symbol, &validated_name);

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let position = params.position;
        let state = self.state.lock().await;

        let file = state
            .open_files
            .get(uri)
            .or_else(|| state.disk_files.get_file(uri));

        let Some(file) = file else {
            return Ok(None);
        };
        let Some((token, span)) = file.token_at(position) else {
            return Ok(None);
        };

        match token {
            sail_parser::Token::Id(name) => Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: Range::new(
                    file.source.position_at(span.start),
                    file.source.position_at(span.end),
                ),
                placeholder: name.clone(),
            })),
            sail_parser::Token::TyVal(name) => {
                Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: Range::new(
                        file.source.position_at(span.start),
                        file.source.position_at(span.end),
                    ),
                    placeholder: format!("'{}", name),
                }))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new_with_client);
    Server::new(stdin, stdout, socket).serve(service).await;
}
