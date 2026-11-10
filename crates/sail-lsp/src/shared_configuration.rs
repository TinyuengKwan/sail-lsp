//! Repository settings are declarative. They cannot authorize subprocesses or
//! replace a user-selected executable. Editors remain the source of that trust.
use crate::config::SailLspConfig;
use lsp_types::Url;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(crate) const FILE_NAME: &str = ".sail-lsp.toml";

#[derive(Default)]
pub(crate) struct Inputs {
    pub global: Value,
    pub folders: BTreeMap<Url, Value>,
    pub last_good: BTreeMap<Url, Value>,
    pub file_errors: BTreeMap<std::path::PathBuf, String>,
}

pub(crate) fn section(value: &Value) -> &Value {
    value.get("sail-lsp").or_else(|| value.get("sail")).unwrap_or(value)
}

pub(crate) fn read(path: &Path) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if text.len() > 256 * 1024 {
        return Err(format!("{}: settings exceed 256 KiB", path.display()));
    }
    let document: toml_edit::DocumentMut =
        text.parse().map_err(|error| format!("{}: {error}", path.display()))?;
    let value =
        table(document.as_table()).map_err(|error| format!("{}: {error}", path.display()))?;
    validate(&value).map_err(|error| format!("{}: {error}", path.display()))?;
    let (_, errors) = SailLspConfig::from_json(&value);
    if !errors.is_empty() {
        return Err(format!("{}: {}", path.display(), errors.join("; ")));
    }
    Ok(Some(value))
}

fn table(table: &toml_edit::Table) -> Result<Value, String> {
    table
        .iter()
        .map(|(key, value)| item(value).map(|value| (key.into(), value)))
        .collect::<Result<serde_json::Map<_, _>, _>>()
        .map(Value::Object)
}
fn item(item: &toml_edit::Item) -> Result<Value, String> {
    match item {
        toml_edit::Item::None => Ok(Value::Null),
        toml_edit::Item::Table(value) => table(value),
        toml_edit::Item::ArrayOfTables(tables) => {
            tables.iter().map(table).collect::<Result<Vec<_>, _>>().map(Value::Array)
        }
        toml_edit::Item::Value(value) => scalar(value),
    }
}
fn scalar(value: &toml_edit::Value) -> Result<Value, String> {
    match value {
        toml_edit::Value::String(value) => Ok(Value::String(value.value().clone())),
        toml_edit::Value::Integer(value) => Ok(Value::from(*value.value())),
        toml_edit::Value::Boolean(value) => Ok(Value::Bool(*value.value())),
        toml_edit::Value::Array(values) => {
            values.iter().map(scalar).collect::<Result<Vec<_>, _>>().map(Value::Array)
        }
        toml_edit::Value::InlineTable(values) => values
            .iter()
            .map(|(key, value)| scalar(value).map(|value| (key.into(), value)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        _ => Err("Settings require strings, integers, booleans, arrays, or tables".into()),
    }
}

fn validate(value: &Value) -> Result<(), String> {
    let defaults = serde_json::to_value(SailLspConfig::default()).unwrap();
    for (section, fields) in value.as_object().unwrap() {
        let Some(known) = defaults.get(section).and_then(Value::as_object) else {
            return Err(format!("Unknown configuration section {section}"));
        };
        if section == "cache" || section == "runnables" {
            return Err(format!(
                "{section} must be configured in the client, not repository settings"
            ));
        }
        let fields = fields.as_object().ok_or_else(|| format!("{section} must be a table"))?;
        for key in fields.keys() {
            if !known.contains_key(key) {
                return Err(format!("Unknown setting {section}.{key}"));
            }
            if (section == "flycheck"
                && matches!(key.as_str(), "enable" | "command" | "extraArgs" | "cacheDirectory"))
                || (section == "workspace" && key == "primeCachesNumThreads")
            {
                return Err(format!(
                    "{section}.{key} must be configured in the client, not repository settings"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn is_config(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_project_values_are_preserved_and_executable_settings_are_not_trusted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        std::fs::write(&path, "[workspace]\nprojectVariables = { RMEM = false, WORD = 'true' }\nincludePaths = ['include']\n[flycheck]\ntimeoutMs = 900000\nignore = ['severity:warning']\n").unwrap();
        let value = read(&path).unwrap().unwrap();
        assert_eq!(value["workspace"]["projectVariables"]["RMEM"], false);
        assert_eq!(value["workspace"]["projectVariables"]["WORD"], "true");
        for text in [
            "[flycheck]\ncommand = 'sh'",
            "[flycheck]\nenable = true",
            "[workspace]\nstrictBitvectro = true",
            "[hover]\ndocs = 42",
            "[completion]\nlimit = 1.5",
        ] {
            std::fs::write(&path, text).unwrap();
            let error = read(&path).unwrap_err();
            assert!(error.contains(&path.display().to_string()), "{text}: {error}");
        }
    }
}
