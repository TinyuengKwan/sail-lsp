//! Server and client capability handling.
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

    /// Whether the client supports `workspace/configuration` requests.
    #[allow(dead_code)]
    pub(crate) fn workspace_configuration_support(&self) -> bool {
        (|| -> Option<bool> { self.0.workspace.as_ref()?.configuration })() == Some(true)
    }

    /// Whether the client supports `window/workDoneProgress` requests.
    #[allow(dead_code)]
    pub(crate) fn work_done_progress_support(&self) -> bool {
        (|| -> Option<bool> { self.0.window.as_ref()?.work_done_progress })() == Some(true)
    }

    /// Whether the client supports `workspace/didChangeWatchedFiles`.
    #[allow(dead_code)]
    pub(crate) fn did_change_watched_files_dynamic_registration(&self) -> bool {
        (|| -> Option<bool> {
            self.0.workspace.as_ref()?.did_change_watched_files.as_ref()?.dynamic_registration
        })() == Some(true)
    }

    /// Whether the client supports code action literals.
    #[allow(dead_code)]
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

    /// Whether the client supports snippet text edits.
    #[allow(dead_code)]
    pub(crate) fn snippet_text_edit_support(&self) -> bool {
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
    #[allow(dead_code)]
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

    /// Whether the client supports semantic tokens delta.
    #[allow(dead_code)]
    pub(crate) fn semantic_tokens_delta_support(&self) -> bool {
        (|| -> Option<bool> {
            let full =
                self.0.text_document.as_ref()?.semantic_tokens.as_ref()?.requests.full.as_ref()?;
            match full {
                lsp_types::SemanticTokensFullOptions::Bool(_) => None,
                lsp_types::SemanticTokensFullOptions::Delta { delta } => *delta,
            }
        })() == Some(true)
    }

    /// Whether the client supports inlay hint resolve.
    #[allow(dead_code)]
    pub(crate) fn inlay_hint_resolve_support(&self) -> bool {
        (|| -> Option<bool> {
            Some(self.0.text_document.as_ref()?.inlay_hint.as_ref()?.resolve_support.is_some())
        })() == Some(true)
    }

    /// Whether the client supports `workspace/willRenameFiles`.
    #[allow(dead_code)]
    pub(crate) fn will_rename_support(&self) -> bool {
        (|| -> Option<bool> { self.0.workspace.as_ref()?.file_operations.as_ref()?.will_rename })()
            == Some(true)
    }
}

/// Advertise server capabilities to the LSP client.
/// Builds ServerCapabilities from config.
pub(crate) fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
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
        })),
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
        ..Default::default()
    }
}
