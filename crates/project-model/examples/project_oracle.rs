//! JSON bridge for scripts/upstream_monitor.py. This is a measurement tool,
//! not a second implementation of project evaluation.
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use project_model::{ProjectManifest, ProjectOptions, ProjectValue};
use serde_json::{json, Value};

fn evaluate(request: &Value) -> Result<Value, String> {
    let text = request["manifest"].as_str().ok_or("manifest must be source text")?;
    let mut options = ProjectOptions::default();
    if let Some(variables) = request.get("variables") {
        for (name, value) in variables.as_object().ok_or("variables must be an object")? {
            let value = match value {
                Value::Bool(b) => ProjectValue::Bool(*b),
                Value::String(s) => ProjectValue::String(s.clone()),
                _ => return Err(format!("unsupported variable {name}: {value}")),
            };
            options.variables.insert(name.clone(), value);
        }
    }
    if let Some(roots) = request.get("roots") {
        options.roots = roots
            .as_array()
            .ok_or("roots must be an array")?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or("root must be a string"))
            .collect::<Result<_, _>>()?;
    }
    match ProjectManifest::parse(text).and_then(|m| m.resolve(&options)) {
        Err(error) => Ok(json!({"accepted": false, "error": error.to_string()})),
        Ok(project) => {
            let requires: BTreeMap<_, Vec<_>> = project
                .modules
                .iter()
                .filter(|m| !m.requires.is_empty())
                .map(|m| {
                    let mut names: Vec<_> =
                        m.requires.iter().map(|&id| &project.modules[id].name).collect();
                    names.sort();
                    (&m.name, names)
                })
                .collect();
            Ok(json!({
                "accepted": true,
                "files": project.file_occurrences().map(|(_, p)| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "modules": project.modules.iter().map(|m| &m.name).collect::<Vec<_>>(),
                "requires": requires,
            }))
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One tool process per request keeps infrastructure failures local and lets
    // the parent enforce a timeout without retaining a stuck worker.
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let request: Value = serde_json::from_str(&line)?;
    let result = evaluate(&request).map_err(io::Error::other)?;
    serde_json::to_writer(io::stdout().lock(), &result)?;
    io::stdout().flush()?;
    Ok(())
}
