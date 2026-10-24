//! Server and client capability handling.
//!
//! Six of these had no caller, and each named something the server
//! assumed: `workspace_configuration_support` (the settings request went
//! out regardless), `hierarchical_symbols_support` (the outline was always
//! sent nested), `work_done_progress_support` (asked a second time by
//! `progress::client_supports_progress`), `code_action_literals_support`
//! and `completion_snippet_support` (code actions and `${1:x}` sent
//! whatever the client said). All six decide something now.
//!
//! Three more were deleted rather than wired, because each guarded
//! nothing: `semantic_tokens_delta_support` and `will_rename_support` —
//! rust-analyzer advertises both capabilities unconditionally and has no
//! such client check, since a client that cannot take them simply does
//! not ask — and `inlay_hint_resolve_support`, because every provider
//! here fills its hint's tooltip when it builds it, so there is nothing
//! left for a resolve to add. RA's use of `will_rename()` is to suppress
//! the double edit a `file_system_edits` rename causes, and Sail has no
//! definition whose rename moves a file.
//!
//! - `pub fn server_capabilities(config: &Config) -> ServerCapabilities`
//! - `impl ClientCapabilities` — client capability queries

use lsp_types::*;

/// Wrapper around LSP `ClientCapabilities` with convenience queries.
/// Provides typed accessors for specific client features rather than
/// requiring callers to navigate the deeply nested LSP struct.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClientCapabilities(pub(crate) lsp_types::ClientCapabilities);

impl ClientCapabilities {
    /// Whether the client supports `relatedDocuments` in diagnostic responses.
    ///.
    pub(crate) fn text_document_diagnostic_related_document_support(&self) -> bool {
        (|| -> Option<bool> {
            self.0.text_document.as_ref()?.diagnostic.as_ref()?.related_document_support
        })() == Some(true)
    }

    /// Whether the client understands a `SnippetTextEdit` in a workspace
    /// edit — rust-analyzer's `experimental.snippetTextEdit`.
    ///
    /// This is what lets an assist place the cursor. It is *not* the
    /// completion capability: a client can expand `${1:x}` in a completion
    /// and still take only plain edits from a code action.
    pub(crate) fn snippet_text_edit_support(&self) -> bool {
        self.0
            .experimental
            .as_ref()
            .and_then(|it| it.get("snippetTextEdit"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    }

    /// Whether the client takes Markdown in a hover.
    ///
    /// `textDocument.hover.contentFormat` is the client's ordered list of
    /// the formats it understands; a client that omits Markdown renders
    /// what it is sent literally, so `**bold**` and backticks arrive as
    /// themselves. The protocol's default when the field is absent is
    /// plaintext only, which is why the absent case answers `false`.
    pub(crate) fn hover_markdown_support(&self) -> bool {
        (|| -> Option<bool> {
            let formats = self.0.text_document.as_ref()?.hover.as_ref()?.content_format.as_ref()?;
            Some(formats.contains(&lsp_types::MarkupKind::Markdown))
        })() == Some(true)
    }

    /// The same question for a completion item's documentation, which the
    /// client answers separately — and does answer differently.
    pub(crate) fn completion_markdown_support(&self) -> bool {
        (|| -> Option<bool> {
            let formats = self
                .0
                .text_document
                .as_ref()?
                .completion
                .as_ref()?
                .completion_item
                .as_ref()?
                .documentation_format
                .as_ref()?;
            Some(formats.contains(&lsp_types::MarkupKind::Markdown))
        })() == Some(true)
    }

    /// Whether the client supports `workspace/configuration` requests.
    pub(crate) fn workspace_configuration_support(&self) -> bool {
        (|| -> Option<bool> { self.0.workspace.as_ref()?.configuration })() == Some(true)
    }

    /// Whether the client supports `window/workDoneProgress` requests.
    pub(crate) fn work_done_progress_support(&self) -> bool {
        (|| -> Option<bool> { self.0.window.as_ref()?.work_done_progress })() == Some(true)
    }

    /// Whether the client supports `workspace/didChangeWatchedFiles`.
    pub(crate) fn did_change_watched_files_dynamic_registration(&self) -> bool {
        (|| -> Option<bool> {
            self.0.workspace.as_ref()?.did_change_watched_files.as_ref()?.dynamic_registration
        })() == Some(true)
    }

    /// Whether the client supports code action literals.
    pub(crate) fn code_action_literals_support(&self) -> bool {
        (|| -> Option<bool> {
            Some(
                self.0
                    .text_document
                    .as_ref()?
                    .code_action
                    .as_ref()?
                    .code_action_literal_support
                    .is_some(),
            )
        })() == Some(true)
    }

    /// Auto-include completion publishes its edit only after resolve.
    pub(crate) fn completion_import_resolve_support(&self) -> bool {
        (|| -> Option<bool> {
            Some(
                self.0
                    .text_document
                    .as_ref()?
                    .completion
                    .as_ref()?
                    .completion_item
                    .as_ref()?
                    .resolve_support
                    .as_ref()?
                    .properties
                    .iter()
                    .any(|property| property == "additionalTextEdits"),
            )
        })() == Some(true)
    }

    /// Whether the client supports snippet text edits.
    pub(crate) fn completion_snippet_support(&self) -> bool {
        (|| -> Option<bool> {
            self.0
                .text_document
                .as_ref()?
                .completion
                .as_ref()?
                .completion_item
                .as_ref()?
                .snippet_support
        })() == Some(true)
    }

    /// Whether the client supports hierarchical document symbols.
    pub(crate) fn hierarchical_symbols_support(&self) -> bool {
        (|| -> Option<bool> {
            self.0
                .text_document
                .as_ref()?
                .document_symbol
                .as_ref()?
                .hierarchical_document_symbol_support
        })() == Some(true)
    }
}

/// Advertise server capabilities to the LSP client.
/// Builds ServerCapabilities from config.
pub(crate) fn server_capabilities(client: &ClientCapabilities) -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        // Only when the client can take a code action *literal*. Without
        // that capability an action has to be sent as a `Command`, which
        // this server has no `workspace/executeCommand` handler for — so
        // advertising the feature would offer actions the client cannot
        // apply. `code_action_literals_support` had no caller until now.
        code_action_provider: client.code_action_literals_support().then(|| {
            CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![
                    CodeActionKind::QUICKFIX,
                    CodeActionKind::REFACTOR,
                    CodeActionKind::REFACTOR_EXTRACT,
                    CodeActionKind::REFACTOR_INLINE,
                    CodeActionKind::REFACTOR_REWRITE,
                    CodeActionKind::SOURCE,
                ]),
                resolve_provider: Some(true),
                work_done_progress_options: Default::default(),
            })
        }),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(DeclarationCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                ".".to_string(),
                ":".to_string(),
                "$".to_string(),
                "@".to_string(),
                "'".to_string(),
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        }),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            super::ext::semantic_tokens_options(),
        )),
        document_highlight_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(true),
            work_done_progress_options: Default::default(),
        }),
        linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
            inter_file_dependencies: true,
            workspace_diagnostics: false,
            ..Default::default()
        })),
        code_lens_provider: Some(CodeLensOptions { resolve_provider: Some(true) }),
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "}".to_string(),
            more_trigger_character: Some(vec![
                ";".to_string(),
                "\n".to_string(),
                "=".to_string(),
                ">".to_string(),
            ]),
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            ..Default::default()
        }),
        workspace: Some(WorkspaceServerCapabilities {
            // Zed gates subsequent folder notifications on this capability.
            // The handler alone is insufficient: roots can arrive after init.
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                will_rename: Some(FileOperationRegistrationOptions {
                    filters: vec![
                        // Match *.sail files.
                        FileOperationFilter {
                            scheme: Some("file".to_string()),
                            pattern: FileOperationPattern {
                                glob: "**/*.sail".to_string(),
                                matches: Some(FileOperationPatternKind::File),
                                options: None,
                            },
                        },
                        // Match directories (moving a folder of .sail files).
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
                ..Default::default()
            }),
            ..Default::default()
        }),
        // The requests that are not in the protocol. A client discovers
        // them here — rust-analyzer advertises its own the same way, and a
        // client that does not see the flag will not send the request.
        experimental: Some(serde_json::json!({
            "matchingBrace": true,
            "joinLines": true,
            "onEnter": true,
            "moveItem": true,
            "ssr": true,
        })),
        ..Default::default()
    }
}
