//! User configuration system.
//!
//! Receives settings via `initialize.initializationOptions` and
//! `workspace/didChangeConfiguration` and
//! controls feature behavior per workspace.
//!
//! Config is structured into sub-configs that are passed
//! to IDE layer functions (e.g., `CompletionConfig`, `HoverConfig`).

use std::collections::HashSet;

use serde::Deserialize;

/// Top-level Sail LSP configuration.
/// has its own sub-config struct.
/// Deserialized from JSON via `workspace/didChangeConfiguration`.
/// Missing fields retain defaults via `#[serde(default)]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct SailLspConfig {
    /// Diagnostics configuration.
    pub diagnostics: DiagnosticsConfig,
    /// Inlay hints configuration.
    pub inlay_hints: InlayHintsConfig,
    /// Completion configuration.
    pub completion: CompletionConfig,
    /// Code lens configuration.
    pub code_lens: CodeLensConfig,
    /// Hover configuration.
    pub hover: HoverConfig,
    /// Semantic tokens configuration.
    pub semantic_tokens: SemanticTokensConfig,
    /// Workspace configuration.
    pub workspace: WorkspaceConfig,
    /// Running upstream Sail beside the server's own analysis.
    pub flycheck: FlycheckConfig,
}

/// `sail --just-check`, as rust-analyzer runs `cargo check`.
///
/// Off by default: it shells out to a binary that may not be installed, and
/// a language server that spawns processes nobody asked for is a surprise.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FlycheckConfig {
    /// Whether to run it at all.
    pub enable: bool,
    /// The binary. `sail` on `PATH` unless the user says otherwise.
    pub command: String,
    /// Arguments before the file list — `--config`, `-D`, an `$include` path.
    pub extra_args: Vec<String>,
}

impl Default for FlycheckConfig {
    fn default() -> Self {
        Self { enable: false, command: "sail".to_string(), extra_args: Vec::new() }
    }
}

/// Diagnostics-related settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticsConfig {
    /// Master enable/disable for diagnostics.
    pub enable: bool,
    /// Report effect mismatches (`effect-violation`, `effect-mismatch`,
    /// `missing-effect-annotation`); `false` disables all three codes.
    pub effect_mismatch: bool,
    /// Diagnostic codes to disable (e.g., "unused-variable").
    pub disabled: HashSet<String>,
    /// Suppress experimental diagnostics.
    pub disable_experimental: bool,
    /// Diagnostic codes to show as hints instead of warnings.
    pub warnings_as_hint: Vec<String>,
    /// Diagnostic codes to show as info instead of warnings.
    pub warnings_as_info: Vec<String>,
    /// Maximum diagnostics per file (0 = unlimited).
    pub max_diagnostics_per_file: usize,
}

/// Inlay hints settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InlayHintsConfig {
    /// Master enable for inlay hints.
    pub enable: bool,
    /// Show type hints for let bindings.
    pub type_hints: bool,
    /// Show parameter name hints at call sites.
    pub parameter_hints: bool,
    /// Show effect annotation hints (Sail-specific).
    pub effect_hints: bool,
    /// Maximum length of inlay hint text before truncation.
    pub max_length: Option<usize>,
    /// Minimum lines for closing brace hints.
    pub closing_brace_hints_min_lines: Option<usize>,
}

/// Completion settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompletionConfig {
    /// Master enable for completions.
    pub enable: bool,
    /// Add parentheses after function completion.
    pub add_call_parenthesis: bool,
    /// Enable postfix completion templates.
    pub postfix: bool,
    /// Enable snippet completions.
    pub snippets: bool,
    /// Enable auto-import ($include) on the fly.
    pub autoimport: bool,
    /// Maximum number of completion items.
    pub limit: usize,
    /// Show full function signatures in completions.
    pub full_function_signatures: bool,
}

/// Code lens settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeLensConfig {
    /// Master enable for code lenses.
    pub enable: bool,
    /// Show reference counts.
    pub references: bool,
    /// Show implementation counts.
    pub implementations: bool,
    /// Show runnable lenses for `main` and `$[test]` functions.
    pub runnables: bool,
}

/// Hover display settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HoverConfig {
    /// Show documentation in hover.
    pub docs: bool,
    /// Maximum fields/union members; null shows all.
    pub max_fields_count: Option<usize>,
    /// Maximum enum variants; null shows all.
    pub max_enum_variants_count: Option<usize>,
    /// Maximum overload alternatives; null shows all.
    pub max_overloads_count: Option<usize>,
}

/// Semantic tokens settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SemanticTokensConfig {
    pub enable: bool,
}

/// Project variables have the same boolean/string values as Sail assignments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ProjectVariableValue {
    Bool(bool),
    String(String),
}

/// Workspace settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceConfig {
    /// Match Sail's --strict-bitvector mode (natural-number vector lengths).
    pub strict_bitvector: bool,
    /// Manifest relative to each workspace root; discovery is used when absent.
    pub project_file: Option<String>,
    /// Overrides for declarations in the Sail project file.
    pub project_variables: std::collections::BTreeMap<String, ProjectVariableValue>,
    /// Selected root modules; empty selects all modules.
    pub project_modules: Vec<String>,
    /// Virtual Sail filenames mapped to generated source paths, relative to
    /// each workspace root (or absolute). The producer owns their contents.
    pub generated_files: std::collections::BTreeMap<String, String>,
    /// Compilation target for `$iftarget` directives.
    pub target: Option<String>,
    /// Additional include paths for `$include` resolution.
    pub include_paths: Vec<String>,
    /// How many threads may pre-warm the caches.
    ///
    /// `0` means "half this machine's cores, at least one" — the
    /// rust-analyzer default. A literal `2` used to be passed at the call
    /// site, so the setting could not exist.
    pub prime_caches_num_threads: usize,
}

impl WorkspaceConfig {
    pub(crate) fn project_options(&self) -> project_model::ProjectOptions {
        project_model::ProjectOptions {
            variables: self
                .project_variables
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        match value {
                            ProjectVariableValue::Bool(value) => {
                                project_model::ProjectValue::Bool(*value)
                            }
                            ProjectVariableValue::String(value) => {
                                project_model::ProjectValue::String(value.clone())
                            }
                        },
                    )
                })
                .collect(),
            roots: self.project_modules.clone(),
            virtual_files: self
                .generated_files
                .iter()
                .map(|(name, path)| (name.clone(), std::path::PathBuf::from(path)))
                .collect(),
        }
    }
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enable: true,
            effect_mismatch: true,
            disabled: HashSet::new(),
            disable_experimental: false,
            warnings_as_hint: Vec::new(),
            warnings_as_info: Vec::new(),
            max_diagnostics_per_file: 128,
        }
    }
}

impl DiagnosticsConfig {
    /// Convert LSP-level config to ide-diagnostics config.
    /// `main_loop.rs` before calling `Analysis::diagnostics()`.
    pub fn to_ide_config(&self) -> ide_diagnostics::DiagnosticsConfig {
        let mut ide_config = ide_diagnostics::DiagnosticsConfig::new();
        ide_config.enabled = self.enable;
        ide_config.disable_experimental = self.disable_experimental;
        // Merge user-disabled codes with defaults.
        ide_config.disabled.extend(self.disabled.iter().cloned());
        if !self.effect_mismatch {
            ide_config.disabled.extend(
                ["effect-violation", "effect-mismatch", "missing-effect-annotation"]
                    .map(String::from),
            );
        }
        ide_config.warnings_as_hint = self.warnings_as_hint.iter().cloned().collect();
        ide_config.warnings_as_info = self.warnings_as_info.iter().cloned().collect();
        ide_config.max_diagnostics_per_file = if self.max_diagnostics_per_file == 0 {
            None
        } else {
            Some(self.max_diagnostics_per_file)
        };
        ide_config
    }
}

impl CompletionConfig {
    /// Convert LSP-level config to ide-completion config.
    pub fn to_ide_config(&self) -> ide::completion::CompletionConfig {
        ide::completion::CompletionConfig {
            enable_postfix_completions: self.postfix,
            enable_keyword_completions: true,
            enable_snippets: self.snippets,
            enable_imports_on_the_fly: self.autoimport,
            limit: if self.limit == 0 { None } else { Some(self.limit) },
            snippet_cap: if self.snippets { ide::completion::SnippetCap::new(true) } else { None },
            full_function_signatures: self.full_function_signatures,
            callable: self
                .add_call_parenthesis
                .then_some(ide::completion::CallableSnippets::FillArguments),
            add_semicolon_to_unit: false,
            fields_to_resolve: ide::completion::CompletionFieldsToResolve::default(),
        }
    }
}

impl Default for InlayHintsConfig {
    fn default() -> Self {
        Self {
            enable: true,
            type_hints: true,
            parameter_hints: true,
            effect_hints: true,
            max_length: Some(25),
            closing_brace_hints_min_lines: Some(6),
        }
    }
}

impl InlayHintsConfig {
    /// Convert LSP-level config to ide inlay hints config.
    pub fn to_ide_config(&self) -> ide::inlay_hints::InlayHintsConfig {
        ide::inlay_hints::InlayHintsConfig {
            max_length: self.max_length,
            type_hints: self.type_hints,
            parameter_hints: self.parameter_hints,
            closing_brace_hints_min_lines: self.closing_brace_hints_min_lines,
            caller_count_hints: false,
            effect_hints: self.effect_hints,
            chaining_hints: false,
            discriminant_hints: false,
        }
    }
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enable: true,
            add_call_parenthesis: true,
            postfix: true,
            snippets: true,
            autoimport: false,
            limit: 200,
            full_function_signatures: false,
        }
    }
}

impl Default for CodeLensConfig {
    fn default() -> Self {
        Self { enable: true, references: true, implementations: true, runnables: true }
    }
}

impl Default for HoverConfig {
    fn default() -> Self {
        Self {
            docs: true,
            max_fields_count: Some(10),
            max_enum_variants_count: Some(10),
            max_overloads_count: Some(10),
        }
    }
}

impl HoverConfig {
    /// Convert LSP-level config to ide hover config.
    pub fn to_ide_config(&self) -> ide::hover::HoverConfig {
        ide::hover::HoverConfig {
            documentation: self.docs,
            max_fields_count: self.max_fields_count,
            max_enum_variants_count: self.max_enum_variants_count,
            max_overloads_count: self.max_overloads_count,
            ..Default::default()
        }
    }
}

impl Default for SemanticTokensConfig {
    fn default() -> Self {
        Self { enable: true }
    }
}

impl SailLspConfig {
    /// The worker count for cache priming, resolved against the machine.
    pub fn prime_caches_num_threads(&self) -> usize {
        match self.workspace.prime_caches_num_threads {
            0 => (std::thread::available_parallelism().map_or(1, usize::from) / 2).max(1),
            n => n,
        }
    }

    pub fn diagnostics_enable(&self) -> bool {
        self.diagnostics.enable
    }
    pub fn effect_mismatch_enable(&self) -> bool {
        self.diagnostics.effect_mismatch
    }
    pub fn inlay_hints_enable(&self) -> bool {
        self.inlay_hints.enable
    }
    pub fn completion_enable(&self) -> bool {
        self.completion.enable
    }
    pub fn code_lenses_enable(&self) -> bool {
        self.code_lens.enable
    }
    pub fn hover_docs_enable(&self) -> bool {
        self.hover.docs
    }
    pub fn semantic_tokens_enable(&self) -> bool {
        self.semantic_tokens.enable
    }
}

impl SailLspConfig {
    /// Parse initialization options or didChangeConfiguration settings,
    /// and name the settings it could not read. Missing keys retain
    /// defaults via serde.
    ///
    /// A setting of the wrong type falls back to its default alone, and is
    /// named in the second element as `section.key: why`. Deserializing the
    /// whole object at once, which this used to do, turned one bad value
    /// into every setting's default, with nothing but a log line to say so.
    pub fn from_json(value: &serde_json::Value) -> (Self, Vec<String>) {
        // Try nested "sail-lsp" section first, then "sail", then raw value.
        let settings = value.get("sail-lsp").or_else(|| value.get("sail")).unwrap_or(value);
        let mut errors = Vec::new();
        let Some(sections) = settings.as_object() else {
            if !settings.is_null() {
                errors.push(format!("settings must be an object, found {settings}"));
            }
            return (Self::default(), errors);
        };
        // Each key is tried on its own, inside its section, so a failure
        // names the one setting it belongs to.
        let accepts = |section: &str, value: serde_json::Value| {
            serde_json::from_value::<Self>(serde_json::json!({ section: value })).err()
        };
        let mut kept = serde_json::Map::new();
        for (section, value) in sections {
            let Some(fields) = value.as_object() else {
                match accepts(section, value.clone()) {
                    None => drop(kept.insert(section.clone(), value.clone())),
                    Some(err) => errors.push(format!("{section}: {err}")),
                }
                continue;
            };
            let mut good = serde_json::Map::new();
            for (key, value) in fields {
                match accepts(section, serde_json::json!({ key: value })) {
                    None => drop(good.insert(key.clone(), value.clone())),
                    Some(err) => errors.push(format!("{section}.{key}: {err}")),
                }
            }
            kept.insert(section.clone(), serde_json::Value::Object(good));
        }
        let config =
            serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_else(|err| {
                errors.push(err.to_string());
                Self::default()
            });
        (config, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = SailLspConfig::default();
        assert!(config.diagnostics.enable);
        assert!(config.inlay_hints.enable);
        assert!(config.inlay_hints.type_hints);
        assert!(config.completion.add_call_parenthesis);
    }

    #[test]
    fn parse_from_json() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": { "enable": false },
                "completion": { "limit": 5 }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(!config.diagnostics.enable);
        assert_eq!(config.completion.limit, 5);
        // Unset fields retain defaults
        assert!(config.inlay_hints.enable);
    }

    #[test]
    fn parse_empty_json() {
        let config = SailLspConfig::from_json(&serde_json::json!({})).0;
        assert!(config.diagnostics.enable); // default
    }

    #[test]
    fn parse_disabled_diagnostics() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": {
                    "disabled": ["unused-variable", "type-mismatch"]
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(config.diagnostics.disabled.contains("unused-variable"));
        assert!(config.diagnostics.disabled.contains("type-mismatch"));
        assert!(!config.diagnostics.disabled.contains("other"));
    }

    #[test]
    fn parse_completion_config() {
        let json = serde_json::json!({
            "sail-lsp": {
                "completion": {
                    "addCallParenthesis": false,
                    "postfix": false
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(!config.completion.add_call_parenthesis);
        assert!(!config.completion.postfix);
        assert!(config.completion.snippets); // default
    }

    #[test]
    fn parse_inlay_hints_config() {
        let json = serde_json::json!({
            "sail-lsp": {
                "inlayHints": {
                    "typeHints": false,
                    "effectHints": false
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(config.inlay_hints.enable); // default
        assert!(!config.inlay_hints.type_hints);
        assert!(config.inlay_hints.parameter_hints); // default
        assert!(!config.inlay_hints.effect_hints);
    }

    #[test]
    fn parse_workspace_config() {
        let json = serde_json::json!({
            "sail-lsp": {
                "workspace": {
                    "includePaths": ["/usr/local/share/sail"],
                    "target": "c"
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert_eq!(config.workspace.include_paths, vec!["/usr/local/share/sail"]);
        assert_eq!(config.workspace.target, Some("c".to_string()));
    }

    #[test]
    fn backward_compat_accessors() {
        let config = SailLspConfig::default();
        assert!(config.diagnostics_enable());
        assert!(config.effect_mismatch_enable());
        assert!(config.inlay_hints_enable());
        assert!(config.completion_enable());
        assert!(config.code_lenses_enable());
        assert!(config.hover_docs_enable());
        assert!(config.semantic_tokens_enable());
    }

    /// One setting of the wrong type costs that setting, not the rest, and
    /// is reported by name.
    #[test]
    fn a_bad_setting_falls_back_alone() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": { "enable": false, "disabled": "unused-variable" },
                "completion": { "limit": "many", "snippets": false },
                "hover": 3
            }
        });
        let (config, errors) = SailLspConfig::from_json(&json);
        assert!(!config.diagnostics.enable, "the good key beside a bad one is kept");
        assert!(!config.completion.snippets);
        assert_eq!(config.completion.limit, 200);
        let mut named: Vec<_> = errors.iter().map(|e| e.split(':').next().unwrap()).collect();
        named.sort_unstable();
        assert_eq!(named, ["completion.limit", "diagnostics.disabled", "hover"], "{errors:?}");
    }

    #[test]
    fn effect_mismatch_false_disables_its_codes() {
        let json = serde_json::json!({ "diagnostics": { "effectMismatch": false } });
        let ide_config = SailLspConfig::from_json(&json).0.diagnostics.to_ide_config();
        assert!(ide_config.disabled.contains("effect-violation"));
        assert!(ide_config.disabled.contains("effect-mismatch"));
        assert!(ide_config.disabled.contains("missing-effect-annotation"));
    }

    #[test]
    fn invalid_json_falls_back_to_defaults() {
        let json = serde_json::json!({ "sail-lsp": "not an object" });
        let config = SailLspConfig::from_json(&json).0;
        assert!(config.diagnostics.enable); // default
    }

    #[test]
    fn default_config_new_fields() {
        let config = SailLspConfig::default();
        // Diagnostics
        assert!(!config.diagnostics.disable_experimental);
        assert!(config.diagnostics.warnings_as_hint.is_empty());
        assert!(config.diagnostics.warnings_as_info.is_empty());
        assert_eq!(config.diagnostics.max_diagnostics_per_file, 128);
        // Completion
        assert!(config.completion.enable);
        assert!(!config.completion.autoimport);
        assert_eq!(config.completion.limit, 200);
        assert!(!config.completion.full_function_signatures);
        // Inlay hints
        assert_eq!(config.inlay_hints.max_length, Some(25));
        assert_eq!(config.inlay_hints.closing_brace_hints_min_lines, Some(6));
        // Hover
        // Code lens
        assert!(config.code_lens.references);
        assert!(config.code_lens.implementations);
    }

    #[test]
    fn parse_new_diagnostics_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": {
                    "disableExperimental": true,
                    "warningsAsHint": ["unused-variable"],
                    "warningsAsInfo": ["type-mismatch"],
                    "maxDiagnosticsPerFile": 50
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(config.diagnostics.disable_experimental);
        assert_eq!(config.diagnostics.warnings_as_hint, vec!["unused-variable"]);
        assert_eq!(config.diagnostics.warnings_as_info, vec!["type-mismatch"]);
        assert_eq!(config.diagnostics.max_diagnostics_per_file, 50);
    }

    #[test]
    fn parse_new_completion_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "completion": {
                    "enable": false,
                    "autoimport": true,
                    "limit": 100,
                    "fullFunctionSignatures": true
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(!config.completion.enable);
        assert!(config.completion.autoimport);
        assert_eq!(config.completion.limit, 100);
        assert!(config.completion.full_function_signatures);
    }

    #[test]
    fn parse_new_inlay_hints_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "inlayHints": {
                    "maxLength": 40,
                    "closingBraceHintsMinLines": 10
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert_eq!(config.inlay_hints.max_length, Some(40));
        assert_eq!(config.inlay_hints.closing_brace_hints_min_lines, Some(10));
    }

    #[test]
    fn parse_new_hover_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "hover": {
                    "docs": false,
                    "maxFieldsCount": 3
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(!config.hover.docs);
        assert_eq!(config.hover.max_fields_count, Some(3));
    }

    #[test]
    fn parse_new_code_lens_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "codeLens": {
                    "references": false,
                    "implementations": false
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        assert!(config.code_lens.enable); // default
        assert!(!config.code_lens.references);
        assert!(!config.code_lens.implementations);
    }

    #[test]
    fn to_ide_diagnostics_config() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": {
                    "enable": true,
                    "disableExperimental": true,
                    "disabled": ["foo"],
                    "warningsAsHint": ["bar"],
                    "maxDiagnosticsPerFile": 0
                }
            }
        });
        let config = SailLspConfig::from_json(&json).0;
        let ide_config = config.diagnostics.to_ide_config();
        assert!(ide_config.enabled);
        assert!(ide_config.disable_experimental);
        assert!(ide_config.disabled.contains("foo"));
        assert!(ide_config.warnings_as_hint.contains("bar"));
        assert!(ide_config.max_diagnostics_per_file.is_none()); // 0 = unlimited
    }

    #[test]
    fn to_ide_completion_config() {
        let config = CompletionConfig {
            enable: true,
            add_call_parenthesis: true,
            postfix: false,
            snippets: true,
            autoimport: true,
            limit: 50,
            full_function_signatures: true,
        };
        let ide_config = config.to_ide_config();
        assert!(!ide_config.enable_postfix_completions);
        assert!(ide_config.enable_imports_on_the_fly);
        assert_eq!(ide_config.limit, Some(50));
        assert!(ide_config.full_function_signatures);
        assert!(ide_config.snippet_cap.is_some());
    }
}
