//! Client settings are resolved once per workspace root, never per query.
use crate::{config::SailLspConfig, global_state::GlobalState, reload::ReloadScope};
use lsp_types::{ConfigurationItem, ConfigurationParams, Url};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct PendingConfiguration {
    pub id: lsp_server::RequestId,
    pub roots: Vec<Url>,
    pub deadline: Instant,
}

/// Match native path components, not URL string prefixes (`model` vs `model2`).
pub(crate) fn for_uri<'a>(
    global: &'a SailLspConfig,
    scopes: &'a BTreeMap<Url, SailLspConfig>,
    uri: &Url,
) -> &'a SailLspConfig {
    let Ok(path) = uri.to_file_path() else { return global };
    scopes
        .iter()
        .filter_map(|(root, config)| {
            let root = root.to_file_path().ok()?;
            path.starts_with(&root).then_some((root.components().count(), config))
        })
        .max_by_key(|(depth, _)| *depth)
        .map_or(global, |(_, config)| config)
}

fn merge(base: &mut serde_json::Value, overrides: &serde_json::Value) {
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        for (key, value) in overrides {
            if let Some(old) = base.get_mut(key) {
                merge(old, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overrides.clone();
    }
}

impl GlobalState {
    pub(crate) fn config_for_uri(&self, uri: &Url) -> &SailLspConfig {
        let selected = self
            .mem_docs
            .get(uri)
            .and_then(|doc| doc.selected_context.as_ref())
            .and_then(|key| serde_json::from_str::<serde_json::Value>(key).ok())
            .and_then(|key| {
                key["module"]["manifest"].as_str().and_then(|value| Url::parse(value).ok())
            })
            .and_then(|uri| uri.to_file_path().ok());
        let project = self
            .projects
            .values()
            .filter(|project| {
                if let Some(selected) = &selected {
                    return &project.manifest == selected;
                }
                project.files.iter().chain(&project.discovered_files).any(|(file, _)| file == uri)
            })
            .min_by_key(|project| {
                let preferred = project.config.active_project.as_ref().is_some_and(|active| {
                    syntax::preprocess::normalize_path(&project.workspace_root.join(active))
                        == project.manifest
                });
                (!preferred, &project.manifest)
            });
        if let Some(config) = project
            .and_then(|project| Url::from_directory_path(&project.workspace_root).ok())
            .and_then(|root| self.scoped_configs.get(&root))
        {
            return config;
        }
        for_uri(&self.config, &self.scoped_configs, uri)
    }

    pub(crate) fn request_configuration(&mut self) {
        self.refresh_shared_configuration();
        if !self.client_caps.workspace_configuration_support() {
            return;
        }
        if let Some(previous) = self.pending_configuration.take() {
            self.req_queue.outgoing.complete(previous.id);
        }
        let mut roots: Vec<_> = self.workspace_folders.iter().cloned().collect();
        roots.sort();
        let items = std::iter::once(ConfigurationItem {
            scope_uri: None,
            section: Some("sail-lsp".into()),
        })
        .chain(roots.iter().map(|uri| ConfigurationItem {
            scope_uri: Some(uri.clone()),
            section: Some("sail-lsp".into()),
        }))
        .collect();
        let request = self.req_queue.outgoing.register(
            <lsp_types::request::WorkspaceConfiguration as lsp_types::request::Request>::METHOD
                .to_owned(),
            ConfigurationParams { items },
            configuration_response as crate::global_state::ReqHandler,
        );
        self.pending_configuration = Some(PendingConfiguration {
            id: request.id.clone(),
            roots,
            deadline: Instant::now() + Duration::from_secs(5),
        });
        self.sender.send(lsp_server::Message::Request(request)).ok();
    }

    pub(crate) fn configuration_timed_out(&mut self) {
        if let Some(previous) = self.pending_configuration.take() {
            self.req_queue.outgoing.complete(previous.id);
        }
        self.log_message(
            lsp_types::MessageType::WARNING,
            "workspace/configuration timed out; using the last available settings.",
        );
    }

    pub(crate) fn apply_configuration_response(
        &mut self,
        roots: Vec<Url>,
        values: Vec<serde_json::Value>,
    ) {
        let Some(global) = values.first() else { return };
        self.configuration_inputs.global = crate::shared_configuration::section(global).clone();
        for (index, root) in roots.into_iter().enumerate() {
            if self.workspace_folders.contains(&root) {
                self.configuration_inputs.folders.insert(
                    root,
                    values
                        .get(index + 1)
                        .map(crate::shared_configuration::section)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
        self.refresh_shared_configuration();
    }

    pub(crate) fn refresh_shared_configuration(&mut self) {
        let (global, mut errors) = SailLspConfig::from_json(&self.configuration_inputs.global);
        self.update_configuration(global);
        self.configuration_inputs.folders.retain(|root, _| self.workspace_folders.contains(root));
        self.configuration_inputs.last_good.retain(|root, _| self.workspace_folders.contains(root));
        let mut scopes = BTreeMap::new();
        let mut file_errors = BTreeMap::new();
        let mut roots: Vec<_> = self.workspace_folders.iter().cloned().collect();
        roots.sort();
        for root in roots {
            let Some(path) =
                root.to_file_path().ok().map(|p| p.join(crate::shared_configuration::FILE_NAME))
            else {
                continue;
            };
            match crate::shared_configuration::read(&path) {
                Ok(Some(value)) => {
                    self.configuration_inputs.last_good.insert(root.clone(), value);
                }
                Ok(None) => {
                    self.configuration_inputs.last_good.remove(&root);
                }
                Err(error) => {
                    file_errors.insert(path, error.clone());
                    errors.push(format!("{error}; keeping the last valid repository settings"));
                }
            }
            let mut merged = self
                .configuration_inputs
                .last_good
                .get(&root)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let client = crate::shared_configuration::section(&self.configuration_inputs.global);
            if client.is_object() {
                merge(&mut merged, client);
            }
            if let Some(value) =
                self.configuration_inputs.folders.get(&root).filter(|v| !v.is_null())
            {
                merge(&mut merged, value);
            }
            let (mut config, scope_errors) = SailLspConfig::from_json(&merged);
            config.cache = self.config.cache.clone();
            config.workspace.prime_caches_num_threads =
                self.config.workspace.prime_caches_num_threads;
            errors.extend(scope_errors.into_iter().map(|error| format!("{root}: {error}")));
            scopes.insert(root, config);
        }
        let previous = std::mem::replace(&mut self.configuration_inputs.file_errors, file_errors);
        for path in previous
            .keys()
            .chain(self.configuration_inputs.file_errors.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let message = self.configuration_inputs.file_errors.get(path);
            if previous.get(path) == message {
                continue;
            }
            let Ok(uri) = Url::from_file_path(path) else { continue };
            let diagnostics = message
                .map(|message| lsp_types::Diagnostic {
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    source: Some("sail-lsp".into()),
                    code: Some(lsp_types::NumberOrString::String("invalid-project-config".into())),
                    message: message.clone(),
                    ..Default::default()
                })
                .into_iter()
                .collect();
            self.send_notification::<lsp_types::notification::PublishDiagnostics>(
                lsp_types::PublishDiagnosticsParams { uri, diagnostics, version: None },
            );
        }
        let changed =
            serde_json::to_value(&*self.scoped_configs).ok() != serde_json::to_value(&scopes).ok();
        let workspace_changed = self
            .scoped_configs
            .iter()
            .map(|(root, c)| (root, &c.workspace))
            .ne(scopes.iter().map(|(root, c)| (root, &c.workspace)));
        let flycheck_changed = self.scoped_configs.len() != scopes.len()
            || scopes.iter().any(|(root, config)| {
                !self
                    .scoped_configs
                    .get(root)
                    .is_some_and(|old| config.flycheck.same_invocation(&old.flycheck))
            });
        let prime_imports = scopes.iter().any(|(root, c)| {
            c.completion.autoimport
                && !self.scoped_configs.get(root).is_some_and(|old| old.completion.autoimport)
        });
        self.semantic_tokens_refresh_requested |= scopes.iter().any(|(root, config)| {
            !self
                .scoped_configs
                .get(root)
                .is_some_and(|old| old.semantic_tokens == config.semantic_tokens)
        });
        self.scoped_configs = Arc::new(scopes);
        if changed {
            self.diagnostics.mark_all_changed();
        }
        if prime_imports {
            self.prime_caches_queue.request_op("scoped autoimport enabled".into(), ());
        }
        if workspace_changed || flycheck_changed {
            self.invalidate_flycheck();
        }
        if workspace_changed {
            self.request_workspace_reload(
                "workspace-scoped configuration changed",
                ReloadScope::All,
            );
        } else if flycheck_changed {
            self.recheck_flycheck();
        }
        self.apply_compiler_diagnostic_policies();
        self.report_config_errors(errors);
    }
}

fn configuration_response(state: &mut GlobalState, response: lsp_server::Response) {
    if !state.pending_configuration.as_ref().is_some_and(|pending| pending.id == response.id) {
        return;
    }
    let pending = state.pending_configuration.take().unwrap();
    match (response.error, response.result) {
        (None, Some(serde_json::Value::Array(values)))
            if values.len() == pending.roots.len() + 1 =>
        {
            state.apply_configuration_response(pending.roots, values)
        }
        (Some(error), _) => state.log_message(
            lsp_types::MessageType::WARNING,
            format!("Failed to fetch workspace settings: {}", error.message),
        ),
        _ => state.log_message(
            lsp_types::MessageType::WARNING,
            "Invalid workspace/configuration response; retaining previous settings.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn state() -> (GlobalState, crossbeam_channel::Receiver<lsp_server::Message>) {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (tasks, _) = crossbeam_channel::unbounded();
        (GlobalState::new(sender, tasks), messages)
    }
    fn root(name: &str) -> Url {
        Url::from_directory_path(std::env::temp_dir().join("sail-scoped-config").join(name))
            .unwrap()
    }
    #[test]
    fn repository_settings_reload_and_explicit_client_values_win_without_losing_last_good_state() {
        let directory = tempfile::tempdir().unwrap();
        let root = Url::from_directory_path(directory.path()).unwrap();
        let path = directory.path().join(crate::shared_configuration::FILE_NAME);
        std::fs::write(&path, "[workspace]\nprojectFile = 'chosen.sail_project'\nstrictBitvector = true\nprojectVariables = { RMEM = false }\n[flycheck]\ntimeoutMs = 900000\n").unwrap();
        let (mut state, _) = state();
        state.workspace_folders.insert(root.clone());
        state.apply_client_settings(&json!({}));
        let config = state.config_for_uri(&root.join("a.sail").unwrap());
        assert!(config.workspace.strict_bitvector);
        assert_eq!(config.flycheck.timeout_ms, 900000);
        state.apply_configuration_response(
            vec![root.clone()],
            vec![
                json!({"workspace":{"strictBitvector":false,"projectFile":null}}),
                json!({"flycheck":{"timeoutMs":800000}}),
            ],
        );
        let config = state.config_for_uri(&root.join("a.sail").unwrap());
        assert!(!config.workspace.strict_bitvector, "explicit false beats repository true");
        assert!(config.workspace.project_file.is_none(), "explicit null clears a repository path");
        assert_eq!(config.flycheck.timeout_ms, 800000);
        std::fs::write(&path, "[workspace]\nprojectVariables = { RMEM = true }\n").unwrap();
        state.handle_vfs_msg(vfs::loader::Message::Changed {
            files: vec![(paths::AbsPathBuf::assert_utf8(path.clone()), std::fs::read(&path).ok())],
        });
        assert_eq!(
            state.config_for_uri(&root.join("a.sail").unwrap()).workspace.project_variables["RMEM"],
            crate::config::ProjectVariableValue::Bool(true)
        );
        std::fs::write(&path, "[workspace\n").unwrap();
        state.refresh_shared_configuration();
        assert!(!state.config_errors.is_empty());
        assert_eq!(
            state.config_for_uri(&root.join("a.sail").unwrap()).workspace.project_variables["RMEM"],
            crate::config::ProjectVariableValue::Bool(true)
        );
        std::fs::remove_file(&path).unwrap();
        state.refresh_shared_configuration();
        assert!(state.config_errors.is_empty());
        assert!(state
            .config_for_uri(&root.join("a.sail").unwrap())
            .workspace
            .project_variables
            .is_empty());
    }
    #[test]
    fn folder_settings_inherit_global_values_and_select_native_path_components() {
        let (mut state, _) = state();
        let a = root("model");
        let b = root("model/nested");
        state.workspace_folders.extend([a.clone(), b.clone()]);
        state.apply_configuration_response(vec![a.clone(), b.clone()], vec![
            json!({"diagnostics":{"enable":true},"flycheck":{"command":"global-sail"},"workspace":{"includePaths":["global"]}}),
            json!({"diagnostics":{"enable":false},"workspace":{"target":"c","includePaths":["local"]}}),
            json!({"flycheck":{"command":"nested-sail"},"workspace":{"target":"ocaml"}}),
        ]);
        let config = state.config_for_uri(&a.join("main.sail").unwrap());
        assert!(!config.diagnostics.enable);
        assert_eq!(config.flycheck.command, "global-sail");
        assert_eq!(config.workspace.include_paths, ["local"]);
        let config = state.config_for_uri(&b.join("main.sail").unwrap());
        assert_eq!(config.flycheck.command, "nested-sail");
        assert_eq!(config.workspace.target.as_deref(), Some("ocaml"));
        assert!(
            state.config_for_uri(&root("model2").join("main.sail").unwrap()).diagnostics.enable
        );
        assert!(state.fetch_workspaces_queue.op_requested());
    }
    #[test]
    fn configuration_requests_are_scoped_and_obsolete_or_timed_out_answers_are_discarded() {
        let (mut state, messages) = state();
        let caps: lsp_types::ClientCapabilities =
            serde_json::from_value(json!({"workspace":{"configuration":true}})).unwrap();
        state.client_caps = crate::lsp::capabilities::ClientCapabilities(caps);
        state.workspace_folders.extend([root("a"), root("b")]);
        state.request_configuration();
        let first = messages
            .try_iter()
            .find_map(|m| if let lsp_server::Message::Request(r) = m { Some(r) } else { None })
            .unwrap();
        assert_eq!(first.params["items"].as_array().unwrap().len(), 3);
        assert!(first.params["items"][0].get("scopeUri").is_none());
        assert_eq!(first.params["items"][1]["scopeUri"], root("a").as_str());
        state.request_configuration();
        state.complete_request(lsp_server::Response::new_ok(
            first.id,
            vec![json!({"diagnostics":{"enable":false}}); 3],
        ));
        assert!(state.config.diagnostics.enable);
        let id = state.pending_configuration.as_ref().unwrap().id.clone();
        state.configuration_timed_out();
        state.complete_request(lsp_server::Response::new_ok(
            id,
            vec![json!({"diagnostics":{"enable":false}}); 3],
        ));
        assert!(state.pending_configuration.is_none());
        assert!(state.config.diagnostics.enable);
    }
    #[test]
    fn shared_files_follow_selected_project_configuration_even_outside_workspace_roots() {
        let (mut state, _) = state();
        let a = root("a");
        let b = root("b");
        let file = root("shared").join("common.sail").unwrap();
        let manifest = |root: &Url| root.join("sail.proj").unwrap().to_file_path().unwrap();
        state.workspace_folders.extend([a.clone(), b.clone()]);
        state.apply_configuration_response(
            vec![a.clone(), b.clone()],
            vec![
                json!({}),
                json!({"flycheck":{"command":"a-sail"}}),
                json!({"flycheck":{"command":"b-sail"}}),
            ],
        );
        state.projects = Arc::new(
            [a.clone(), b.clone()]
                .into_iter()
                .map(|root| {
                    (
                        root.clone(),
                        crate::reload::FolderProject {
                            manifest: manifest(&root),
                            workspace_root: root.to_file_path().unwrap(),
                            files: vec![(file.clone(), file.to_file_path().unwrap())],
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        );
        assert_eq!(state.config_for_uri(&file).flycheck.command, "a-sail");
        state.mem_docs.insert(file.clone(), crate::mem_docs::DocumentData::new(1, String::new()));
        state.mem_docs.get_mut(&file).unwrap().selected_context =
            Some(json!({"module":{"manifest":b.join("sail.proj").unwrap()}}).to_string());
        assert_eq!(state.config_for_uri(&file).flycheck.command, "b-sail");
    }
}
