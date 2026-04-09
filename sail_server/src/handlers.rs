use std::collections::hash_map::HashMap;

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
    SelectionRangeProviderCapability, SemanticTokensDeltaParams, SemanticTokensFullDeltaResult,
    SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensRangeResult,
    SemanticTokensResult, ServerCapabilities, SignatureHelp, SignatureHelpOptions,
    SignatureHelpParams, SymbolInformation, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, TypeDefinitionProviderCapability,
    TypeHierarchyItem, TypeHierarchyOptions, TypeHierarchyPrepareParams,
    TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, Url, WatchKind,
    WorkDoneProgressOptions, WorkspaceDiagnosticParams, WorkspaceDiagnosticReportResult,
    WorkspaceEdit, WorkspaceFileOperationsServerCapabilities, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities, WorkspaceSymbol, WorkspaceSymbolOptions, WorkspaceSymbolParams,
};
use tower_lsp::LanguageServer;

use crate::actions::{
    add_missing_match_arms_edits, apply_demorgan_edits, block_to_line_comment_edits,
    code_action_kind_allowed, default_code_action_format_options, extract_function_edits,
    extract_local_let_edits, flip_binexpr_edits, generate_doc_template_edits,
    guarded_return_edits, inline_variable_edits, invert_if_edits, lazy_code_action_data,
    line_to_block_comment_edits, organize_imports_edits, pull_assignment_up_edits,
    quick_fix_for_diagnostic, remove_unused_imports_edits, resolve_code_action_edit_from_data,
    sail_source_fix_all_kind, sort_items_edits, toggle_doc_comment_edits, unused_variable_fix,
    unwrap_block_edits, var_to_let_fix, bitfield_accessor_edits,
};
use crate::backend::{should_schedule_typecheck, Backend, SAIL_BUILTINS, SAIL_KEYWORDS};
use crate::completion::{
    build_completion_items, completion_prefix, completion_trigger_characters,
    postfix_completions, pragma_completions, resolve_completion_item, snippet_completions,
};
use crate::diagnostics::{document_diagnostic_report_for_file, workspace_diagnostic_report};
use crate::formatting::{
    document_links_for_file, format_document_edits, join_lines_edits,
    linked_editing_ranges_for_position, make_selection_range, matching_brace_position,
    move_item_edits, on_enter_edits, range_format_document_edits, MoveDirection,
};
use crate::hover::hover_for_symbol;
use crate::inlay_hints::{inlay_hints_for_range, resolve_inlay_hint};
use crate::semantic_tokens::{
    compute_semantic_tokens, compute_semantic_tokens_delta, compute_semantic_tokens_range,
    semantic_tokens_options,
};
use crate::state::File;
use crate::symbols::{
    call_edges_from, call_edges_to, call_hierarchy_item, code_lens_title, code_lenses_for_file,
    collect_implementation_counts, collect_reference_counts, extract_symbol_decls,
    find_call_at_position, find_callable_signature, implementation_locations,
    normalize_validated_rename, parse_named_type, reference_locations, rename_edits,
    resolve_symbol_at, resolve_workspace_symbol, signature_help_for_position,
    symbol_declaration_locations, symbol_definition_locations, symbol_spans_for_file,
    token_is_close_bracket, token_is_open_bracket, token_symbol_key, type_definition_locations,
    type_hierarchy_item, type_name_candidates_at_position, type_subtypes, type_supertypes,
    typed_bindings, will_rename_file_edits,
};

/// Fuzzy match: each query character must appear in name (in order).
/// Score rewards: consecutive matches, word-boundary matches, prefix matches.
fn fuzzy_match_score(query: &str, name: &str) -> Option<i32> {
    let mut score = 0i32;
    let mut name_iter = name.char_indices().peekable();
    let mut prev_matched = false;
    let mut first = true;

    for qch in query.chars() {
        let mut found = false;
        while let Some((idx, nch)) = name_iter.next() {
            if nch == qch {
                // Consecutive match bonus
                if prev_matched {
                    score += 5;
                }
                // Word boundary bonus (after '_' or uppercase)
                if idx == 0
                    || name.as_bytes().get(idx.wrapping_sub(1)) == Some(&b'_')
                    || (nch.is_ascii_lowercase()
                        && idx > 0
                        && name.as_bytes()[idx - 1].is_ascii_uppercase())
                {
                    score += 3;
                }
                // Prefix match bonus
                if first && idx == 0 {
                    score += 10;
                }
                prev_matched = true;
                first = false;
                found = true;
                break;
            }
            prev_matched = false;
        }
        if !found {
            return None;
        }
    }
    Some(score)
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.client
            .log_message(MessageType::INFO, "server initialized")
            .await;

        {
            let mut state = self.state.write().await;
            if let Some(workspace_folders) = params.workspace_folders {
                for folder in workspace_folders {
                    state.disk_files.add_folder(folder.uri);
                }
            }
        }
        self.schedule_workspace_scan().await;

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
                            CodeActionKind::REFACTOR_INLINE,
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
                    first_trigger_character: "\n".to_string(),
                    more_trigger_character: Some(vec![
                        "}".to_string(),
                        ";".to_string(),
                        "=".to_string(),
                        ">".to_string(),
                    ]),
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
                    commands: vec![
                        "sail.noop".to_string(),
                        "sail.run".to_string(),
                        "sail.matchingBrace".to_string(),
                        "sail.joinLines".to_string(),
                        "sail.moveItemUp".to_string(),
                        "sail.moveItemDown".to_string(),
                    ],
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

        {
            let mut state = self.state.write().await;

            for folder in params.event.added.iter() {
                state.disk_files.add_folder(folder.uri.clone());
            }
            for folder in params.event.removed.iter() {
                state.disk_files.remove_folder(&folder.uri);
            }
        }

        self.schedule_workspace_scan().await;
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

        let mut state = self.state.write().await;
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
                                let file = File::new_lazy(source);
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
        let file = File::new_lazy(params.text_document.text);
        let typecheck_file = file.clone();
        {
            let mut state = self.state.write().await;
            state.diagnostic_versions.insert(uri.clone(), version);
            state.open_files.insert(uri.clone(), file);
        }
        self.schedule_debounced_diagnostics(uri.clone(), version);
        if should_schedule_typecheck(&typecheck_file) {
            self.schedule_debounced_typecheck(uri, version, typecheck_file);
        }
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

        let typecheck_file = {
            let mut state = self.state.write().await;

            let file = state
                .open_files
                .get_mut(uri)
                .expect("document changed that isn't open");
            file.update(params.content_changes);
            let typecheck_file = file.clone();
            state.diagnostic_versions.insert(uri.clone(), version);
            typecheck_file
        };
        self.schedule_debounced_diagnostics(uri.clone(), version);
        if should_schedule_typecheck(&typecheck_file) {
            self.schedule_debounced_typecheck(uri.clone(), version, typecheck_file);
        }
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

        let mut state = self.state.write().await;
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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

        let state = self.state.read().await;
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

        let state = self.state.read().await;
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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

        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };

        // Check if the cursor is on a control-flow keyword for enhanced highlighting
        if let Some((token, _span)) = file.token_at(position) {
            let related = match token {
                sail_parser::Token::KwReturn
                | sail_parser::Token::KwThrow
                | sail_parser::Token::KwExit => {
                    // Highlight all exit points in the containing function
                    highlight_exit_points(file, position)
                }
                sail_parser::Token::KwForeach
                | sail_parser::Token::KwWhile
                | sail_parser::Token::KwRepeat => {
                    // Highlight loop keyword and all break-like expressions in the loop body
                    highlight_loop_points(file, position)
                }
                sail_parser::Token::KwIf | sail_parser::Token::KwElse => {
                    // Highlight all if/else keywords in the same if-else chain
                    highlight_branch_points(file, position)
                }
                sail_parser::Token::KwMatch => {
                    // Highlight match keyword and all case arrows
                    highlight_match_arms(file, position)
                }
                _ => None,
            };
            if let Some(highlights) = related {
                if !highlights.is_empty() {
                    return Ok(Some(highlights));
                }
            }
        }

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

        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };

        let include_declaration = params.context.include_declaration;
        let Some(symbol) = resolve_symbol_at(file, position) else {
            return Ok(None);
        };
        let locations = reference_locations(state.all_files(), uri, &symbol, include_declaration);

        Ok(Some(locations))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };

        let tree = crate::symbols::analysis::document_symbol_tree(file);
        Ok(Some(DocumentSymbolResponse::Nested(tree)))
    }

    #[allow(deprecated)]
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.to_ascii_lowercase();
        let state = self.state.read().await;
        let mut scored: Vec<(i32, SymbolInformation)> = Vec::new();

        for (uri, file) in state.all_files() {
            for decl in extract_symbol_decls(file) {
                let score = if query.is_empty() {
                    0
                } else {
                    match fuzzy_match_score(&query, &decl.name.to_ascii_lowercase()) {
                        Some(s) => s,
                        None => continue,
                    }
                };
                let range = Range::new(
                    file.source.position_at(decl.offset),
                    file.source.position_at(decl.offset + decl.name.len()),
                );
                scored.push((
                    score,
                    SymbolInformation {
                        name: decl.name,
                        kind: decl.kind,
                        tags: None,
                        deprecated: None,
                        location: Location::new(uri.clone(), range),
                        container_name: Some(decl.detail.to_string()),
                    },
                ));
            }
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(200);
        Ok(Some(scored.into_iter().map(|(_, sym)| sym).collect()))
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        let state = self.state.read().await;
        Ok(resolve_workspace_symbol(params, state.all_files()))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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

        let state = self.state.read().await;
        let edges = call_edges_to(state.all_files(), &target_name);
        let mut grouped: HashMap<String, (Url, Vec<Range>)> = HashMap::new();
        for edge in edges {
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

        let state = self.state.read().await;
        let edges = call_edges_from(state.all_files(), &caller_name);
        let mut grouped: HashMap<String, Vec<Range>> = HashMap::new();
        for edge in edges {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };
        let Some(tokens) = file.tokens.as_deref() else {
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

        // Fold consecutive line comments (// ...) and consecutive $include directives.
        let text = file.source.text();
        let lines: Vec<&str> = text.split('\n').collect();
        let mut group_start: Option<(u32, &str)> = None; // (line, kind)

        for (line_idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let kind = if trimmed.starts_with("//") {
                Some("comment")
            } else if trimmed.starts_with("$include") {
                Some("import")
            } else {
                None
            };

            match (&group_start, kind) {
                (Some((start, gk)), Some(k)) if *gk == k => {
                    // Continue group — update end implicitly via line_idx.
                    if line_idx == lines.len() - 1 && line_idx as u32 > *start {
                        ranges.push(FoldingRange {
                            start_line: *start,
                            start_character: None,
                            end_line: line_idx as u32,
                            end_character: None,
                            kind: Some(if *gk == "comment" {
                                FoldingRangeKind::Comment
                            } else {
                                FoldingRangeKind::Imports
                            }),
                            collapsed_text: None,
                        });
                    }
                }
                (Some((start, gk)), _) => {
                    let end = (line_idx as u32).saturating_sub(1);
                    if end > *start {
                        ranges.push(FoldingRange {
                            start_line: *start,
                            start_character: None,
                            end_line: end,
                            end_character: None,
                            kind: Some(if *gk == "comment" {
                                FoldingRangeKind::Comment
                            } else {
                                FoldingRangeKind::Imports
                            }),
                            collapsed_text: None,
                        });
                    }
                    group_start = kind.map(|k| (line_idx as u32, k));
                }
                (None, Some(k)) => {
                    group_start = Some((line_idx as u32, k));
                }
                (None, None) => {}
            }
        }

        // Fold /* ... */ block comments.
        {
            let mut block_start: Option<u32> = None;
            for (line_idx, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("/*") && !trimmed.contains("*/") {
                    block_start = Some(line_idx as u32);
                } else if trimmed.contains("*/") {
                    if let Some(start) = block_start {
                        let end = line_idx as u32;
                        if end > start {
                            ranges.push(FoldingRange {
                                start_line: start,
                                start_character: None,
                                end_line: end,
                                end_character: None,
                                kind: Some(FoldingRangeKind::Comment),
                                collapsed_text: Some("/* ... */".to_string()),
                            });
                        }
                        block_start = None;
                    }
                }
            }
        }

        // Fold consecutive /// doc comment lines.
        {
            let mut doc_start: Option<u32> = None;
            for (line_idx, line) in lines.iter().enumerate() {
                let is_doc = line.trim().starts_with("///");
                match (doc_start, is_doc) {
                    (None, true) => doc_start = Some(line_idx as u32),
                    (Some(start), false) => {
                        let end = (line_idx as u32).saturating_sub(1);
                        if end > start {
                            ranges.push(FoldingRange {
                                start_line: start,
                                start_character: None,
                                end_line: end,
                                end_character: None,
                                kind: Some(FoldingRangeKind::Comment),
                                collapsed_text: Some("/// ...".to_string()),
                            });
                        }
                        doc_start = None;
                    }
                    _ => {}
                }
            }
            if let Some(start) = doc_start {
                let end = (lines.len() as u32).saturating_sub(1);
                if end > start {
                    ranges.push(FoldingRange {
                        start_line: start,
                        start_character: None,
                        end_line: end,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Comment),
                        collapsed_text: Some("/// ...".to_string()),
                    });
                }
            }
        }

        // AST-based structural folding: functions, match arms, definitions.
        if let Some(core_ast) = file.core_ast.as_deref() {
            for (def, def_span) in &core_ast.defs {
                let start_line = file.source.position_at(def_span.start).line;
                let end_line = file.source.position_at(def_span.end).line;
                if end_line > start_line + 1 {
                    ranges.push(FoldingRange {
                        start_line,
                        start_character: None,
                        end_line,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }

                // Fold match arms within callable bodies
                if let sail_parser::core_ast::DefinitionKind::Callable(c) = &def.kind {
                    let bodies: Vec<&(sail_parser::core_ast::Expr, sail_parser::Span)> = c
                        .body
                        .iter()
                        .chain(c.clauses.iter().filter_map(|(cl, _)| cl.body.as_ref()))
                        .collect();
                    for body in bodies {
                        collect_match_fold_ranges(body, file, &mut ranges);
                    }
                }
            }
        }

        Ok(Some(ranges))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        self.with_file(&params.text_document.uri, |_, file| {
            let ranges = params
                .positions
                .iter()
                .map(|pos| make_selection_range(file, *pos))
                .collect::<Vec<_>>();
            Some(ranges)
        })
        .await
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
            if let Some((title, edit, is_preferred)) = var_to_let_fix(file, diagnostic) {
                let kind = CodeActionKind::QUICKFIX;
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
            if let Some((title, edit, is_preferred)) = unused_variable_fix(file, diagnostic) {
                let kind = CodeActionKind::QUICKFIX;
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

        // "Fix all X" actions: group fixes by diagnostic code.
        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::QUICKFIX) {
            let mut by_code: HashMap<String, Vec<TextEdit>> = HashMap::new();
            for diagnostic in &params.context.diagnostics {
                let code_str = diagnostic.code.as_ref().and_then(|c| match c {
                    tower_lsp::lsp_types::NumberOrString::String(s) => Some(s.clone()),
                    _ => None,
                });
                let Some(code_str) = code_str else { continue };
                let edit = quick_fix_for_diagnostic(file, diagnostic)
                    .map(|(_, e, _)| e)
                    .or_else(|| var_to_let_fix(file, diagnostic).map(|(_, e, _)| e))
                    .or_else(|| unused_variable_fix(file, diagnostic).map(|(_, e, _)| e));
                if let Some(edit) = edit {
                    by_code.entry(code_str).or_default().push(edit);
                }
            }
            for (code, edits) in &by_code {
                if edits.len() > 1 {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Fix all '{}' ({} fixes)", code, edits.len()),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: None,
                        edit: None,
                        command: None,
                        is_preferred: Some(false),
                        disabled: None,
                        data: Some(lazy_code_action_data(uri, edits)),
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
            if let Some(edits) = extract_function_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Extract selection to function".to_string(),
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

        // Refactor: Invert If, Flip Binary Expression, De Morgan, Inline Variable, Generate Doc
        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::REFACTOR_REWRITE) {
            if let Some(edits) = invert_if_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Invert if/else branches".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = flip_binexpr_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Flip binary expression operands".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = apply_demorgan_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Apply De Morgan's law".to_string(),
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

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::REFACTOR_INLINE) {
            if let Some(edits) = inline_variable_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Inline variable".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_INLINE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
        }

        // New refactors: unwrap block, pull assignment up, guarded return, sort items,
        // comment conversions, add missing match arms
        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::REFACTOR_REWRITE) {
            if let Some(edits) = unwrap_block_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Unwrap block".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = pull_assignment_up_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Pull assignment up into expression".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = guarded_return_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Convert to guarded return".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = sort_items_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Sort members alphabetically".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = line_to_block_comment_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Convert to block comment".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = block_to_line_comment_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Convert to line comments".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = toggle_doc_comment_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Toggle doc comment".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) =
                add_missing_match_arms_edits(file, params.range, state.all_files())
            {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Add missing match arms".to_string(),
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

        if code_action_kind_allowed(&requested_kinds, &CodeActionKind::SOURCE) {
            if let Some(edits) = bitfield_accessor_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Generate bitfield accessor functions".to_string(),
                    kind: Some(CodeActionKind::SOURCE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: Some(false),
                    disabled: None,
                    data: Some(lazy_code_action_data(uri, &edits)),
                }));
            }
            if let Some(edits) = generate_doc_template_edits(file, params.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Generate documentation template".to_string(),
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
            if let Some(edits) = remove_unused_imports_edits(file) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Remove unused $include directives".to_string(),
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

        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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

        // Add postfix completions (e.g. expr.if, expr.match, expr.let)
        items.extend(postfix_completions(file.source.text(), offset, prefix));

        // Add pragma completions (when after @ or $)
        items.extend(pragma_completions(file.source.text(), offset));

        // Add snippet completions (code templates)
        let is_top_level = {
            let mut depth = 0i32;
            for b in file.source.text()[..offset].bytes() {
                match b {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
            }
            depth <= 0
        };
        items.extend(snippet_completions(prefix, is_top_level));

        if items.is_empty() {
            return Ok(None);
        }

        Ok(Some(CompletionResponse::Array(std::mem::take(&mut items))))
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        let state = self.state.read().await;
        item = resolve_completion_item(item, state.all_files());
        Ok(item)
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;

        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let mut state = self.state.write().await;
        let Some(file) = state.get_file(uri) else {
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
        let mut state = self.state.write().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };
        Ok(Some(
            compute_semantic_tokens_range(file, &params.range).into(),
        ))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        self.with_file(&params.text_document.uri, |_, file| {
            format_document_edits(file, &params.options)
        })
        .await
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document_position.text_document.uri;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
            return Ok(None);
        };

        match params.ch.as_str() {
            "}" | ";" => Ok(format_document_edits(file, &params.options)),
            "\n" => Ok(on_enter_edits(file, params.text_document_position.position)),
            "=" => {
                // After typing `=`, check if it completes `=>` (fat arrow) — auto-indent
                let pos = params.text_document_position.position;
                let offset = file.source.offset_at(&pos);
                let text = file.source.text();
                // If user just typed `>` after `=` forming `=>`, reformat the line
                if offset >= 2 && text.as_bytes().get(offset - 2) == Some(&b'=') {
                    Ok(format_document_edits(file, &params.options))
                } else {
                    Ok(None)
                }
            }
            ">" => {
                // After typing `>`, check if it forms `->` or `=>` — reformat
                let pos = params.text_document_position.position;
                let offset = file.source.offset_at(&pos);
                let text = file.source.text();
                if offset >= 2
                    && matches!(
                        text.as_bytes().get(offset - 2),
                        Some(b'-') | Some(b'=')
                    )
                {
                    Ok(format_document_edits(file, &params.options))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.with_file(&params.text_document.uri, |_, file| {
            range_format_document_edits(file, params.range, &params.options)
        })
        .await
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let position = params.text_document_position_params.position;
        self.with_file(
            &params.text_document_position_params.text_document.uri,
            |_, file| linked_editing_ranges_for_position(file, position),
        )
        .await
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;
        let Some(file) = state.get_file(uri) else {
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
            }
            "sail.matchingBrace" => {
                if let Some(result) = self.handle_matching_brace(&params.arguments).await {
                    return Ok(Some(result));
                }
            }
            "sail.joinLines" => {
                self.handle_join_lines(&params.arguments).await;
            }
            "sail.moveItemUp" => {
                self.handle_move_item(&params.arguments, MoveDirection::Up)
                    .await;
            }
            "sail.moveItemDown" => {
                self.handle_move_item(&params.arguments, MoveDirection::Down)
                    .await;
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
        let state = self.state.read().await;
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
        let state = self.state.read().await;

        let Some(file) = state.get_file(uri) else {
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
        let state = self.state.read().await;

        let Some(file) = state.get_file(uri) else {
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

// ---------------------------------------------------------------------------
// Highlight Related helpers
// ---------------------------------------------------------------------------

use sail_parser::core_ast::{
    BlockItem as CoreBlockItem, DefinitionKind as CoreDefinitionKind,
    Expr as CoreExpr, SourceFile as CoreSourceFile,
};

fn highlight_exit_points(file: &File, position: tower_lsp::lsp_types::Position) -> Option<Vec<DocumentHighlight>> {
    let offset = file.source.offset_at(&position);
    let core_ast = file.core_ast.as_deref()?;

    // Find the containing function body
    let body = find_containing_function_body(core_ast, offset)?;
    let mut spans = Vec::new();
    collect_exit_spans(&body.0, &mut spans);

    let highlights = spans
        .into_iter()
        .map(|span| DocumentHighlight {
            range: Range::new(
                file.source.position_at(span.start),
                file.source.position_at(span.end),
            ),
            kind: Some(DocumentHighlightKind::TEXT),
        })
        .collect();
    Some(highlights)
}

fn collect_exit_spans(expr: &CoreExpr, spans: &mut Vec<sail_parser::Span>) {
    match expr {
        CoreExpr::Return(e) => {
            // Highlight just the "return" keyword area (first few chars of the span)
            // We'll highlight the return expression span
            spans.push(e.1);
        }
        CoreExpr::Throw(e) => {
            spans.push(e.1);
        }
        CoreExpr::Exit(_) => {
            // exit is an exit point
        }
        CoreExpr::Block(items) => {
            for item in items {
                match &item.0 {
                    CoreBlockItem::Expr(e) | CoreBlockItem::Var { value: e, .. } => {
                        collect_exit_spans(&e.0, spans);
                    }
                    CoreBlockItem::Let(lb) => collect_exit_spans(&lb.value.0, spans),
                }
            }
        }
        CoreExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_exit_spans(&cond.0, spans);
            collect_exit_spans(&then_branch.0, spans);
            if let Some(e) = else_branch {
                collect_exit_spans(&e.0, spans);
            }
        }
        CoreExpr::Match { cases, .. } | CoreExpr::Try { cases, .. } => {
            for case in cases {
                if let Some(body) = &case.0.guard {
                    collect_exit_spans(&body.0, spans);
                }
                collect_exit_spans(&case.0.body.0, spans);
            }
        }
        CoreExpr::Let { body, .. } | CoreExpr::Var { body, .. } => {
            collect_exit_spans(&body.0, spans);
        }
        CoreExpr::Foreach(f) => collect_exit_spans(&f.body.0, spans),
        CoreExpr::While { body, .. } | CoreExpr::Repeat { body, .. } => {
            collect_exit_spans(&body.0, spans);
        }
        _ => {}
    }
}

fn find_containing_function_body<'a>(
    ast: &'a CoreSourceFile,
    offset: usize,
) -> Option<&'a (CoreExpr, sail_parser::Span)> {
    for (def, _) in &ast.defs {
        match &def.kind {
            CoreDefinitionKind::Callable(c) => {
                if let Some(body) = &c.body {
                    if body.1.start <= offset && offset <= body.1.end {
                        return Some(body);
                    }
                }
                for clause in &c.clauses {
                    if let Some(body) = &clause.0.body {
                        if body.1.start <= offset && offset <= body.1.end {
                            return Some(body);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn highlight_loop_points(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<Vec<DocumentHighlight>> {
    // For now, highlight the loop keyword and its body brackets
    let tokens = file.tokens.as_deref()?;
    let offset = file.source.offset_at(&position);

    // Find the loop keyword token
    let (_token, span) = tokens
        .iter()
        .find(|(_, s)| s.start <= offset && offset < s.end)?;

    let mut highlights = vec![DocumentHighlight {
        range: Range::new(
            file.source.position_at(span.start),
            file.source.position_at(span.end),
        ),
        kind: Some(DocumentHighlightKind::TEXT),
    }];

    // Find the loop body in the core AST and highlight exit points within
    let core_ast = file.core_ast.as_deref()?;
    if let Some(body) = find_loop_body_at(core_ast, offset) {
        let mut exit_spans = Vec::new();
        collect_exit_spans(&body.0, &mut exit_spans);
        for s in exit_spans {
            highlights.push(DocumentHighlight {
                range: Range::new(
                    file.source.position_at(s.start),
                    file.source.position_at(s.end),
                ),
                kind: Some(DocumentHighlightKind::TEXT),
            });
        }
    }

    Some(highlights)
}

fn find_loop_body_at<'a>(
    ast: &'a CoreSourceFile,
    offset: usize,
) -> Option<&'a (CoreExpr, sail_parser::Span)> {
    for (def, _) in &ast.defs {
        if let CoreDefinitionKind::Callable(c) = &def.kind {
            if let Some(body) = &c.body {
                if let Some(r) = find_loop_in_expr(body, offset) {
                    return Some(r);
                }
            }
            for clause in &c.clauses {
                if let Some(body) = &clause.0.body {
                    if let Some(r) = find_loop_in_expr(body, offset) {
                        return Some(r);
                    }
                }
            }
        }
    }
    None
}

fn find_loop_in_expr<'a>(
    expr: &'a (CoreExpr, sail_parser::Span),
    offset: usize,
) -> Option<&'a (CoreExpr, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        CoreExpr::Foreach(f) => Some(&f.body),
        CoreExpr::While { body, .. } | CoreExpr::Repeat { body, .. } => Some(body),
        CoreExpr::Block(items) => items.iter().find_map(|item| match &item.0 {
            CoreBlockItem::Expr(e) | CoreBlockItem::Var { value: e, .. } => {
                find_loop_in_expr(e, offset)
            }
            CoreBlockItem::Let(lb) => find_loop_in_expr(&*lb.value, offset),
        }),
        CoreExpr::If {
            then_branch,
            else_branch,
            ..
        } => find_loop_in_expr(then_branch, offset).or_else(|| {
            else_branch
                .as_ref()
                .and_then(|e| find_loop_in_expr(e, offset))
        }),
        CoreExpr::Let { body, .. } | CoreExpr::Var { body, .. } => {
            find_loop_in_expr(body, offset)
        }
        CoreExpr::Match { cases, .. } | CoreExpr::Try { cases, .. } => {
            cases.iter().find_map(|c| {
                find_loop_in_expr(&c.0.body, offset)
            })
        }
        _ => None,
    }
}

fn highlight_branch_points(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<Vec<DocumentHighlight>> {
    // Highlight all if/else keywords in the same chain
    let offset = file.source.offset_at(&position);

    let core_ast = file.core_ast.as_deref()?;
    let body = find_containing_function_body(core_ast, offset)?;
    let if_spans = find_if_chain_keyword_spans(&body.0, offset, file.source.text())?;

    let highlights = if_spans
        .into_iter()
        .map(|span| DocumentHighlight {
            range: Range::new(
                file.source.position_at(span.start),
                file.source.position_at(span.end),
            ),
            kind: Some(DocumentHighlightKind::TEXT),
        })
        .collect::<Vec<_>>();

    if highlights.is_empty() {
        None
    } else {
        Some(highlights)
    }
}

fn find_if_chain_keyword_spans(
    _expr: &CoreExpr,
    _offset: usize,
    _text: &str,
) -> Option<Vec<sail_parser::Span>> {
    None // TODO: requires keyword span tracking in core AST
}

fn highlight_match_arms(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<Vec<DocumentHighlight>> {
    let offset = file.source.offset_at(&position);
    let core_ast = file.core_ast.as_deref()?;

    // Find the match expression
    let body = find_containing_function_body(core_ast, offset)?;
    let match_expr = find_match_at(body, offset)?;

    let cases = match &match_expr.0 {
        CoreExpr::Match { cases, .. } | CoreExpr::Try { cases, .. } => cases,
        _ => return None,
    };

    let mut highlights = Vec::new();
    highlights.push(DocumentHighlight {
        range: Range::new(
            file.source.position_at(match_expr.1.start),
            file.source.position_at(match_expr.1.start + 5), // "match" length
        ),
        kind: Some(DocumentHighlightKind::TEXT),
    });

    // Highlight each case body span
    for case in cases {
        highlights.push(DocumentHighlight {
            range: Range::new(
                file.source.position_at(case.0.pattern.1.start),
                file.source.position_at(case.0.pattern.1.end),
            ),
            kind: Some(DocumentHighlightKind::TEXT),
        });
    }

    Some(highlights)
}

fn find_match_at<'a>(
    expr: &'a (CoreExpr, sail_parser::Span),
    offset: usize,
) -> Option<&'a (CoreExpr, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        CoreExpr::Match { .. } | CoreExpr::Try { .. } => {
            // Check if offset is near the start (on the keyword)
            if offset < expr.1.start + 10 {
                return Some(expr);
            }
            None
        }
        CoreExpr::Block(items) => items.iter().find_map(|item| match &item.0 {
            CoreBlockItem::Expr(e) | CoreBlockItem::Var { value: e, .. } => {
                find_match_at(e, offset)
            }
            CoreBlockItem::Let(lb) => find_match_at(&*lb.value, offset),
        }),
        CoreExpr::If {
            then_branch,
            else_branch,
            ..
        } => find_match_at(then_branch, offset).or_else(|| {
            else_branch
                .as_ref()
                .and_then(|e| find_match_at(e, offset))
        }),
        CoreExpr::Let { body, .. } | CoreExpr::Var { body, .. } => {
            find_match_at(body, offset)
        }
        CoreExpr::Foreach(f) => find_match_at(&f.body, offset),
        CoreExpr::While { body, .. } | CoreExpr::Repeat { body, .. } => {
            find_match_at(body, offset)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Folding range helpers
// ---------------------------------------------------------------------------

fn collect_match_fold_ranges(
    expr: &(CoreExpr, sail_parser::Span),
    file: &File,
    ranges: &mut Vec<FoldingRange>,
) {
    match &expr.0 {
        CoreExpr::Match { cases, .. } | CoreExpr::Try { cases, .. } => {
            for (case, case_span) in cases {
                let start = file.source.position_at(case_span.start).line;
                let end = file.source.position_at(case_span.end).line;
                if end > start {
                    ranges.push(FoldingRange {
                        start_line: start,
                        start_character: None,
                        end_line: end,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
                collect_match_fold_ranges(&case.body, file, ranges);
            }
        }
        CoreExpr::Block(items) => {
            for item in items {
                match &item.0 {
                    CoreBlockItem::Expr(e) | CoreBlockItem::Var { value: e, .. } => {
                        collect_match_fold_ranges(e, file, ranges);
                    }
                    CoreBlockItem::Let(lb) => {
                        collect_match_fold_ranges(&*lb.value, file, ranges);
                    }
                }
            }
        }
        CoreExpr::If { then_branch, else_branch, .. } => {
            collect_match_fold_ranges(then_branch, file, ranges);
            if let Some(e) = else_branch {
                collect_match_fold_ranges(e, file, ranges);
            }
        }
        CoreExpr::Let { body, .. } | CoreExpr::Var { body, .. } => {
            collect_match_fold_ranges(body, file, ranges);
        }
        CoreExpr::Foreach(f) => collect_match_fold_ranges(&f.body, file, ranges),
        CoreExpr::While { body, .. } | CoreExpr::Repeat { body, .. } => {
            collect_match_fold_ranges(body, file, ranges);
        }
        _ => {}
    }
}

// Helper methods for custom commands
impl Backend {
    /// Parse a { uri, position } from command arguments.
    fn parse_uri_position(
        args: &[serde_json::Value],
    ) -> Option<(Url, tower_lsp::lsp_types::Position)> {
        let obj = args.first()?;
        let uri_str = obj.get("uri")?.as_str()?;
        let uri = Url::parse(uri_str).ok()?;
        let pos = obj.get("position")?;
        let line = pos.get("line")?.as_u64()? as u32;
        let character = pos.get("character")?.as_u64()? as u32;
        Some((uri, tower_lsp::lsp_types::Position::new(line, character)))
    }

    fn parse_uri_range(
        args: &[serde_json::Value],
    ) -> Option<(Url, Range)> {
        let obj = args.first()?;
        let uri_str = obj.get("uri")?.as_str()?;
        let uri = Url::parse(uri_str).ok()?;
        let start = obj.get("start").or_else(|| obj.get("position"))?;
        let end = obj.get("end").unwrap_or(start);
        let start_line = start.get("line")?.as_u64()? as u32;
        let start_char = start.get("character")?.as_u64()? as u32;
        let end_line = end.get("line")?.as_u64()? as u32;
        let end_char = end.get("character")?.as_u64()? as u32;
        Some((
            uri,
            Range::new(
                tower_lsp::lsp_types::Position::new(start_line, start_char),
                tower_lsp::lsp_types::Position::new(end_line, end_char),
            ),
        ))
    }

    async fn handle_matching_brace(
        &self,
        args: &[serde_json::Value],
    ) -> Option<serde_json::Value> {
        let (uri, position) = Self::parse_uri_position(args)?;
        let state = self.state.read().await;
        let file = state.get_file(&uri)?;
        let pos = matching_brace_position(file, position)?;
        Some(serde_json::json!({
            "line": pos.line,
            "character": pos.character,
        }))
    }

    async fn handle_join_lines(&self, args: &[serde_json::Value]) {
        let Some((uri, range)) = Self::parse_uri_range(args) else {
            return;
        };
        let state = self.state.read().await;
        let Some(file) = state.get_file(&uri) else {
            return;
        };
        let Some(edits) = join_lines_edits(file, range) else {
            return;
        };
        drop(state);
        let mut changes = HashMap::new();
        changes.insert(uri, edits);
        let _ = self
            .client
            .apply_edit(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            })
            .await;
    }

    async fn handle_move_item(
        &self,
        args: &[serde_json::Value],
        direction: MoveDirection,
    ) {
        let Some((uri, position)) = Self::parse_uri_position(args) else {
            return;
        };
        let state = self.state.read().await;
        let Some(file) = state.get_file(&uri) else {
            return;
        };
        let Some(edits) = move_item_edits(file, position, direction) else {
            return;
        };
        drop(state);
        let mut changes = HashMap::new();
        changes.insert(uri, edits);
        let _ = self
            .client
            .apply_edit(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            })
            .await;
    }
}
