use zed::lsp::{Completion, CompletionKind};
use zed::{CodeLabel, CodeLabelSpan};
use zed_extension_api::settings::LspSettings;
use zed_extension_api::{self as zed, serde_json, Result};

const LSP_SETTINGS_KEYS: [&str; 2] = ["sail-lsp", "sail_lsp"];

fn settings_keys(language_server_id: &zed::LanguageServerId) -> Vec<String> {
    let mut keys = vec![language_server_id.as_ref().to_string()];
    for key in LSP_SETTINGS_KEYS {
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_string());
        }
    }
    keys
}

struct SailExtension;

impl zed::Extension for SailExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let mut env = worktree.shell_env();
        let mut user_command = None;
        let mut user_args = Vec::new();

        for key in settings_keys(language_server_id) {
            let Some(binary) = LspSettings::for_worktree(&key, worktree)
                .ok()
                .and_then(|settings| settings.binary)
            else {
                continue;
            };

            if let Some(user_env) = binary.env {
                env.extend(user_env);
            }
            if let Some(path) = binary.path {
                user_command = Some(path);
            }
            if let Some(args) = binary.arguments {
                user_args = args;
            }
            break;
        }

        if let Some(path) = user_command {
            return Ok(zed::Command {
                command: path,
                args: user_args,
                env,
            });
        }

        if let Some(path) = worktree.which("sail-lsp") {
            return Ok(zed::Command {
                command: path,
                args: user_args,
                env,
            });
        }

        Err("sail-lsp is not installed. Run the sail-lsp installer with --editor zed, then reload this extension. For remote projects, install the server on the remote host.".into())
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        for key in settings_keys(language_server_id) {
            if let Ok(lsp_settings) = LspSettings::for_worktree(&key, worktree) {
                if lsp_settings.initialization_options.is_some() {
                    return Ok(lsp_settings.initialization_options);
                }
            }
        }
        Ok(None)
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        for key in settings_keys(language_server_id) {
            if let Ok(lsp_settings) = LspSettings::for_worktree(&key, worktree) {
                if lsp_settings.settings.is_some() {
                    return Ok(lsp_settings.settings);
                }
            }
        }
        Ok(None)
    }

    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: Completion,
    ) -> Option<CodeLabel> {
        let name = completion.label;

        match completion.kind {
            Some(CompletionKind::Function | CompletionKind::Method) => {
                let detail = completion
                    .detail
                    .unwrap_or_else(|| "() -> unit".to_string());
                let head = "val ";
                let middle = " : ";
                let code = format!("{head}{name}{middle}{detail}");
                let name_start = head.len();
                let detail_start = name_start + name.len() + middle.len();

                Some(CodeLabel {
                    code,
                    spans: vec![
                        CodeLabelSpan::code_range(name_start..name_start + name.len()),
                        CodeLabelSpan::code_range(detail_start..detail_start + detail.len()),
                    ],
                    filter_range: (0..name.len()).into(),
                })
            }
            Some(
                CompletionKind::TypeParameter
                | CompletionKind::Class
                | CompletionKind::Enum
                | CompletionKind::EnumMember,
            ) => Some(CodeLabel {
                code: String::new(),
                spans: vec![CodeLabelSpan::literal(&name, Some("type".to_string()))],
                filter_range: (0..name.len()).into(),
            }),
            Some(CompletionKind::Keyword) => Some(CodeLabel {
                code: String::new(),
                spans: vec![CodeLabelSpan::literal(&name, Some("keyword".to_string()))],
                filter_range: (0..name.len()).into(),
            }),
            Some(CompletionKind::Constant | CompletionKind::Value | CompletionKind::Variable) => {
                if let Some(detail) = completion.detail {
                    let middle = " : ";
                    let code = format!("{name}{middle}{detail}");
                    let detail_start = name.len() + middle.len();
                    Some(CodeLabel {
                        code,
                        spans: vec![
                            CodeLabelSpan::code_range(0..name.len()),
                            CodeLabelSpan::code_range(detail_start..detail_start + detail.len()),
                        ],
                        filter_range: (0..name.len()).into(),
                    })
                } else {
                    Some(CodeLabel {
                        code: name.clone(),
                        spans: vec![CodeLabelSpan::code_range(0..name.len())],
                        filter_range: (0..name.len()).into(),
                    })
                }
            }
            _ => None,
        }
    }
}

zed::register_extension!(SailExtension);
