//! User configuration system.
//!
//! Receives settings via `workspace/didChangeConfiguration` and
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
    /// Z3 solver configuration (Sail-specific).
    pub z3: Z3Config,
    /// Workspace configuration.
    pub workspace: WorkspaceConfig,
}

/// Diagnostics-related settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticsConfig {
    /// Master enable/disable for diagnostics.
    pub enable: bool,
    /// Enable effect mismatch warnings.
    pub effect_mismatch: bool,
    /// Diagnostic codes to disable (e.g., "unused-variable").
    pub disabled: HashSet<String>,
    /// Suppress experimental diagnostics.
    pub disable_experimental: bool,
    /// Diagnostic codes to show as hints instead of warnings.
    pub warnings_as_hint: Vec<String>,
    /// Diagnostic codes to show as info instead of warnings.
    pub warnings_as_info: Vec<String>,
    /// Path prefix remapping for diagnostics.
    pub remap_prefix: std::collections::HashMap<String, String>,
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
    /// Enable private/editable symbol completions.
    pub private_editable: bool,
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
    /// Show keyword documentation in hover.
    pub keywords: bool,
    /// Hover actions (go-to-definition, etc.).
    pub actions: bool,
}

/// Semantic tokens settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SemanticTokensConfig {
    pub enable: bool,
}

/// Z3 SMT solver settings (Sail-specific).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Z3Config {
    /// Timeout in milliseconds (0 = no timeout).
    pub timeout_ms: u32,
}

/// Workspace settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceConfig {
    /// Maximum number of files to scan.
    pub max_files: usize,
    /// Compilation target for `$iftarget` directives.
    pub target: Option<String>,
    /// Additional include paths for `$include` resolution.
    pub include_paths: Vec<String>,
}

impl Default for SailLspConfig {
    fn default() -> Self {
        Self {
            diagnostics: DiagnosticsConfig::default(),
            inlay_hints: InlayHintsConfig::default(),
            completion: CompletionConfig::default(),
            code_lens: CodeLensConfig::default(),
            hover: HoverConfig::default(),
            semantic_tokens: SemanticTokensConfig::default(),
            z3: Z3Config::default(),
            workspace: WorkspaceConfig::default(),
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
            remap_prefix: std::collections::HashMap::new(),
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
        ide_config.warnings_as_hint = self.warnings_as_hint.iter().cloned().collect();
        ide_config.warnings_as_info = self.warnings_as_info.iter().cloned().collect();
        ide_config.remap_prefix = self.remap_prefix.clone();
        ide_config.max_diagnostics_per_file = if self.max_diagnostics_per_file == 0 {
            None
        } else {
            Some(self.max_diagnostics_per_file)
        };
        ide_config
    }
}

#[allow(dead_code)]
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
            callable: Some(ide::completion::CallableSnippets::FillArguments),
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

#[allow(dead_code)]
impl InlayHintsConfig {
    /// Convert LSP-level config to ide inlay hints config.
    pub fn to_ide_config(&self) -> ide::inlay_hints::InlayHintsConfig {
        ide::inlay_hints::InlayHintsConfig {
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
            private_editable: false,
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
        Self { docs: true, keywords: true, actions: true }
    }
}

#[allow(dead_code)]
impl HoverConfig {
    /// Convert LSP-level config to ide hover config.
    pub fn to_ide_config(&self) -> ide_db::ide_types::HoverConfig {
        ide_db::ide_types::HoverConfig {
            documentation: self.docs,
            keywords: self.keywords,
            ..Default::default()
        }
    }
}

impl Default for SemanticTokensConfig {
    fn default() -> Self {
        Self { enable: true }
    }
}

impl Default for Z3Config {
    fn default() -> Self {
        Self { timeout_ms: 1000 }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self { max_files: 10000, target: None, include_paths: Vec::new() }
    }
}

#[allow(dead_code)]
impl SailLspConfig {
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
    pub fn z3_timeout_ms(&self) -> u32 {
        self.z3.timeout_ms
    }
    pub fn max_workspace_files(&self) -> usize {
        self.workspace.max_files
    }
}

impl SailLspConfig {
    /// Parse configuration from a JSON value (from didChangeConfiguration).
    /// flat JSON. Missing keys retain defaults via serde.
    pub fn from_json(value: &serde_json::Value) -> Self {
        // Try nested "sail-lsp" section first, then "sail", then raw value.
        let settings = value.get("sail-lsp").or_else(|| value.get("sail")).unwrap_or(value);

        // Deserialize with serde — missing fields get defaults.
        match serde_json::from_value::<SailLspConfig>(settings.clone()) {
            Ok(config) => config,
            Err(err) => {
                log::warn!("failed to parse config, using defaults: {err}");
                SailLspConfig::default()
            }
        }
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
        assert_eq!(config.z3.timeout_ms, 1000);
    }

    #[test]
    fn parse_from_json() {
        let json = serde_json::json!({
            "sail-lsp": {
                "diagnostics": { "enable": false },
                "z3": { "timeoutMs": 5000 }
            }
        });
        let config = SailLspConfig::from_json(&json);
        assert!(!config.diagnostics.enable);
        assert_eq!(config.z3.timeout_ms, 5000);
        // Unset fields retain defaults
        assert!(config.inlay_hints.enable);
    }

    #[test]
    fn parse_empty_json() {
        let config = SailLspConfig::from_json(&serde_json::json!({}));
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
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
        assert_eq!(config.z3_timeout_ms(), 1000);
        assert_eq!(config.max_workspace_files(), 10000);
    }

    #[test]
    fn invalid_json_falls_back_to_defaults() {
        let json = serde_json::json!({ "sail-lsp": "not an object" });
        let config = SailLspConfig::from_json(&json);
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
        assert!(!config.completion.private_editable);
        // Inlay hints
        assert_eq!(config.inlay_hints.max_length, Some(25));
        assert_eq!(config.inlay_hints.closing_brace_hints_min_lines, Some(6));
        // Hover
        assert!(config.hover.keywords);
        assert!(config.hover.actions);
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
        assert_eq!(config.inlay_hints.max_length, Some(40));
        assert_eq!(config.inlay_hints.closing_brace_hints_min_lines, Some(10));
    }

    #[test]
    fn parse_new_hover_fields() {
        let json = serde_json::json!({
            "sail-lsp": {
                "hover": {
                    "docs": true,
                    "keywords": false,
                    "actions": false
                }
            }
        });
        let config = SailLspConfig::from_json(&json);
        assert!(config.hover.docs);
        assert!(!config.hover.keywords);
        assert!(!config.hover.actions);
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
        let config = SailLspConfig::from_json(&json);
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
        let config = SailLspConfig::from_json(&json);
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
            private_editable: false,
        };
        let ide_config = config.to_ide_config();
        assert!(!ide_config.enable_postfix_completions);
        assert!(ide_config.enable_imports_on_the_fly);
        assert_eq!(ide_config.limit, Some(50));
        assert!(ide_config.full_function_signatures);
        assert!(ide_config.snippet_cap.is_some());
    }
}
