//! User configuration system.
//!
//! Receives settings via `initialize.initializationOptions` and
//! `workspace/didChangeConfiguration` and
//! controls feature behavior per workspace.
//!
//! Config is structured into sub-configs that are passed
//! to IDE layer functions (e.g., `CompletionConfig`, `HoverConfig`).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Top-level Sail LSP configuration.
/// has its own sub-config struct.
/// Deserialized from JSON via `workspace/didChangeConfiguration`.
/// Missing fields retain defaults via `#[serde(default)]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct SailLspConfig {
    pub formatting: FormattingConfig,
    /// Retained analysis query values and optional process memory budget.
    pub cache: CacheConfig,
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
    /// Explicit project test/export commands exposed to editors.
    pub runnables: RunnablesConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunnablesConfig {
    /// Timeout for the standard LSP task fallback (native clients own their tasks).
    pub timeout_ms: u64,
    pub tasks: Vec<ProjectTaskConfig>,
    pub show_code_lens: bool,
}
impl Default for RunnablesConfig {
    fn default() -> Self {
        Self { tasks: Vec::new(), show_code_lens: true, timeout_ms: 300_000 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectTaskConfig {
    pub label: String,
    pub kind: ProjectTaskKind,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    /// Manifest path relative to the workspace root; absent means every project.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectTaskKind {
    Test,
    Export,
}

/// Capacities count query results, not bytes. Zero disables that family's LRU.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CacheConfig {
    pub parse_capacity: usize,
    pub environment_capacity: usize,
    pub inference_capacity: usize,
    pub body_capacity: usize,
    /// Soft RSS high-water mark; zero disables automatic pressure control.
    pub memory_limit_mb: u32,
}
impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            parse_capacity: 0,
            environment_capacity: 0,
            inference_capacity: 0,
            body_capacity: 512,
            memory_limit_mb: 0,
        }
    }
}
impl CacheConfig {
    pub(crate) fn capacities(&self) -> ide_db::cache::QueryCacheCapacities {
        ide_db::cache::QueryCacheCapacities {
            parse: self.parse_capacity,
            environment: self.environment_capacity,
            inference: self.inference_capacity,
            bodies: self.body_capacity,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormattingConfig {
    /// Soft line width; indivisible expressions and aligned tables may exceed it.
    pub line_width: u32,
}
impl Default for FormattingConfig {
    fn default() -> Self {
        Self { line_width: 100 }
    }
}

/// `sail --just-check`, as rust-analyzer runs `cargo check`.
///
/// Off by default: it shells out to a binary that may not be installed, and
/// a language server that spawns processes nobody asked for is a surprise.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FlycheckConfig {
    pub diagnostic_format: CompilerDiagnosticFormat,
    pub cache: CompilerCacheMode,
    /// Managed cache root; relative paths use the compiler working directory.
    pub cache_directory: Option<String>,
    /// Presentation of compiler findings, independent of native diagnostics.
    pub ignore: Vec<String>,
    pub warnings_as_hint: Vec<String>,
    pub warnings_as_info: Vec<String>,
    pub timeout_ms: u64,
    pub debounce_ms: u64,
    /// Whether to run it at all.
    pub enable: bool,
    /// The binary. `sail` on `PATH` unless the user says otherwise.
    pub command: String,
    /// Extra compiler arguments — `--config`, `--strict-bitvector`, an `$include` path.
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerCacheMode {
    #[default]
    Managed,
    Compiler,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerDiagnosticFormat {
    #[default]
    Auto,
    Text,
    Json,
}

impl Default for FlycheckConfig {
    fn default() -> Self {
        Self {
            diagnostic_format: CompilerDiagnosticFormat::Auto,
            cache: CompilerCacheMode::Managed,
            cache_directory: None,
            ignore: Vec::new(),
            warnings_as_hint: Vec::new(),
            warnings_as_info: Vec::new(),
            enable: false,
            command: "sail".to_string(),
            extra_args: Vec::new(),
            timeout_ms: 600_000,
            debounce_ms: 200,
        }
    }
}

impl FlycheckConfig {
    /// Presentation changes do not alter or restart a compiler invocation.
    pub(crate) fn same_invocation(&self, other: &Self) -> bool {
        fn invocation(config: &FlycheckConfig) -> FlycheckConfig {
            let mut config = config.clone();
            config.ignore.clear();
            config.warnings_as_hint.clear();
            config.warnings_as_info.clear();
            config
        }
        invocation(self) == invocation(other)
    }
}

/// Diagnostics-related settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// Types of intermediate field-access expressions.
    pub chaining_hints: bool,
    /// Values of automatically generated Sail enum conversions.
    pub enum_value_hints: bool,
    /// Maximum length of inlay hint text before truncation.
    pub max_length: Option<usize>,
    /// Minimum lines for closing brace hints.
    pub closing_brace_hints_min_lines: Option<usize>,
}

/// Completion settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompletionConfig {
    pub custom_templates: Vec<CompletionTemplateConfig>,
    pub term_search: bool,
    pub term_search_fuel: usize,
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

/// Templates use LSP snippet syntax; `${receiver}` is reserved for postfix.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(try_from = "RawCompletionTemplate")]
pub struct CompletionTemplateConfig {
    pub prefix: Option<String>,
    pub postfix: Option<String>,
    #[serde(serialize_with = "serialize_template_body")]
    pub body: std::sync::Arc<str>,
    pub description: String,
    pub scope: CompletionTemplateScope,
}
fn serialize_template_body<S: serde::Serializer>(
    body: &std::sync::Arc<str>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(body)
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionTemplateScope {
    Item,
    #[default]
    Expr,
    Type,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCompletionTemplate {
    prefix: Option<String>,
    postfix: Option<String>,
    body: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    scope: CompletionTemplateScope,
}
impl TryFrom<RawCompletionTemplate> for CompletionTemplateConfig {
    type Error = String;
    fn try_from(raw: RawCompletionTemplate) -> Result<Self, String> {
        if raw.prefix.is_none() && raw.postfix.is_none() {
            return Err("a template needs prefix or postfix".into());
        }
        for trigger in raw.prefix.iter().chain(&raw.postfix) {
            if trigger.is_empty()
                || trigger.len() > 128
                || !trigger.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                return Err(
                    "template triggers must contain 1..128 bytes of letters, digits or underscores"
                        .into(),
                );
            }
        }
        if raw.body.len() > 65536 {
            return Err("template body exceeds 64 KiB".into());
        }
        if raw.postfix.is_some() && !matches!(raw.scope, CompletionTemplateScope::Expr) {
            return Err("postfix templates require expr scope".into());
        }
        Ok(Self {
            prefix: raw.prefix,
            postfix: raw.postfix,
            body: raw.body.into(),
            description: raw.description,
            scope: raw.scope,
        })
    }
}

/// Code lens settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeLensConfig {
    /// Master enable for code lenses.
    pub enable: bool,
    /// Show reference counts.
    pub references: bool,
    /// Show implementation counts.
    pub implementations: bool,
}

/// Hover display settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SemanticTokensConfig {
    pub enable: bool,
}

/// A manifest variable's value. A JSON boolean is a boolean and a JSON
/// string is a string, even `"true"`: unlike `sail --variable`, which reads a
/// bare `true`/`false` as a boolean, the JSON type says which one is meant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ProjectVariableValue {
    Bool(bool),
    String(String),
}

/// Explicit project entries. Paths are relative to the containing workspace
/// folder; each detailed entry can override manifest variables and roots.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LinkedProject {
    Path(String),
    Config(LinkedProjectConfig),
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedProjectConfig {
    pub project_file: String,
    #[serde(default)]
    pub project_variables: std::collections::BTreeMap<String, ProjectVariableValue>,
    pub project_modules: Option<Vec<String>>,
}
impl LinkedProject {
    pub fn path(&self) -> &str {
        match self {
            Self::Path(path) => path,
            Self::Config(c) => &c.project_file,
        }
    }
    pub fn effective(&self, base: &WorkspaceConfig) -> WorkspaceConfig {
        let mut config = base.clone();
        config.linked_projects.clear();
        config.project_file = Some(self.path().to_owned());
        if let Self::Config(overrides) = self {
            config.project_variables.extend(overrides.project_variables.clone());
            if let Some(roots) = &overrides.project_modules {
                config.project_modules = roots.clone();
            }
        }
        config
    }
}

/// Workspace settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceConfig {
    pub linked_projects: Vec<LinkedProject>,
    /// Preferred project for files shared by multiple projects.
    pub active_project: Option<String>,
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
            custom_templates: self
                .custom_templates
                .iter()
                .take(128)
                .map(|template| ide::completion::CompletionTemplate {
                    prefix: template.prefix.clone(),
                    postfix: template.postfix.clone(),
                    body: template.body.clone(),
                    description: template.description.clone(),
                    scope: match template.scope {
                        CompletionTemplateScope::Item => ide::completion::SnippetScope::Item,
                        CompletionTemplateScope::Expr => ide::completion::SnippetScope::Expr,
                        CompletionTemplateScope::Type => ide::completion::SnippetScope::Type,
                    },
                })
                .collect::<Vec<_>>()
                .into(),
            term_search: self.term_search,
            term_search_fuel: self.term_search_fuel.min(10000),
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
            chaining_hints: true,
            enum_value_hints: false,
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
            chaining_hints: self.chaining_hints,
            discriminant_hints: self.enum_value_hints,
        }
    }
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            custom_templates: Vec::new(),
            term_search: false,
            term_search_fuel: 1000,
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
        Self { enable: true, references: true, implementations: true }
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
    /// Preserve task structure in reports without exporting environment values.
    pub(crate) fn diagnostic_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("configuration is JSON serializable");
        if let Some(tasks) = value["runnables"]["tasks"].as_array_mut() {
            for task in tasks {
                if let Some(environment) = task["environment"].as_object_mut() {
                    for value in environment.values_mut() {
                        *value = serde_json::Value::String("<redacted>".into());
                    }
                }
            }
        }
        value
    }

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
        let mut config: Self = serde_json::from_value(serde_json::Value::Object(kept))
            .unwrap_or_else(|err| {
                errors.push(err.to_string());
                Self::default()
            });
        if config.completion.custom_templates.len() > 128 {
            errors.push("completion.customTemplates: at most 128 templates are supported".into());
            config.completion.custom_templates.truncate(128);
        }
        if config.completion.term_search_fuel > 10000 {
            errors.push("completion.termSearchFuel: capped at 10000 units".into());
            config.completion.term_search_fuel = 10000;
        }
        (config, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the VS Code extension sends — the whole `sail-lsp` section, as
    /// `package.json` declares it — is read without a complaint, so the
    /// extension's settings and the server's cannot drift apart silently.
    #[test]
    fn the_extension_settings_section_is_understood() {
        let package: serde_json::Value =
            serde_json::from_str(include_str!("../../../editors/code/package.json")).unwrap();
        let properties = package["contributes"]["configuration"]["properties"].as_object().unwrap();
        let mut section = serde_json::Map::new();
        for (key, schema) in properties {
            let (group, field) = key.strip_prefix("sail-lsp.").unwrap().split_once('.').unwrap();
            let group = section.entry(group).or_insert_with(|| serde_json::json!({}));
            group[field] = schema["default"].clone();
        }
        let (config, errors) = SailLspConfig::from_json(&serde_json::Value::Object(section));
        assert_eq!(errors, Vec::<String>::new());
        assert!(!config.flycheck.enable);
    }

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
            ..Default::default()
        };
        let ide_config = config.to_ide_config();
        assert!(!ide_config.enable_postfix_completions);
        assert!(ide_config.enable_imports_on_the_fly);
        assert_eq!(ide_config.limit, Some(50));
        assert!(ide_config.full_function_signatures);
        assert!(ide_config.snippet_cap.is_some());
    }
}

#[cfg(test)]
mod template_config_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn invalid_templates_are_reported_without_losing_other_completion_settings() {
        for template in [
            json!({"body":"body"}),
            json!({"prefix":"","body":"body"}),
            json!({"postfix":"wrap","body":"body","scope":"type"}),
            json!({"prefix":"p","body":"body","unknown":true}),
        ] {
            let (config, errors) = SailLspConfig::from_json(
                &json!({"completion":{"customTemplates":[template],"limit":17}}),
            );
            assert_eq!(config.completion.limit, 17);
            assert!(config.completion.custom_templates.is_empty());
            assert!(
                errors.iter().any(|error| error.contains("completion.customTemplates")),
                "{errors:?}"
            );
        }
        let (config, errors) = SailLspConfig::from_json(
            &json!({"completion":{"customTemplates":[{"postfix":"wrap","body":"wrap(${receiver})$0"}],"termSearch":true,"termSearchFuel":7}}),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let ide = config.completion.to_ide_config();
        assert!(ide.term_search);
        assert_eq!(ide.term_search_fuel, 7);
        assert!(std::sync::Arc::ptr_eq(
            &config.completion.custom_templates[0].body,
            &ide.custom_templates[0].body
        ));
        assert!(std::sync::Arc::ptr_eq(&ide.custom_templates, &ide.clone().custom_templates));
    }
}
