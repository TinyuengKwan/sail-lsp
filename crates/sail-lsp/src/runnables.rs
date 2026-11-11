//! Project-scoped discovery shared by native editor tasks and the standard
//! LSP command fallback. Compiler checks
//! go through Flycheck's negotiated protocol, cancellation and diagnostic queue.
use std::collections::{BTreeMap, BTreeSet, HashSet};

use lsp_types::{TextDocumentIdentifier, Url};
use serde_json::json;

use crate::config::ProjectTaskConfig;
use crate::global_state::GlobalStateSnapshot;
use crate::lsp_ext::{FlycheckControlParams, ProcessRunnableArgs, Runnable, RunnableCommand};
use crate::reload::FolderProject;

fn scalar(input: &str, bindings: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or("Unclosed task placeholder")?;
        let name = &after[..end];
        output.push_str(
            bindings
                .get(name)
                .ok_or_else(|| format!("Unknown or misplaced task placeholder: ${{{name}}}"))?,
        );
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    if output.contains('\0') {
        return Err("Task arguments cannot contain NUL".into());
    }
    Ok(output)
}

fn process(
    project: &FolderProject,
    compiler: &str,
    task: &ProjectTaskConfig,
    roots: &HashSet<Url>,
) -> Result<ProcessRunnableArgs, String> {
    if !task.args.iter().any(|a| a == "${projectArgs}" || a.contains("${projectContext}"))
        && !task.environment.values().any(|v| v.contains("${projectContext}"))
    {
        return Err("Task must receive ${projectArgs} or ${projectContext} to preserve the loaded project configuration".into());
    }
    let cwd = project.manifest.parent().ok_or("Project manifest has no directory")?.to_path_buf();
    let includes = crate::reload::include_search_paths(roots, &project.config);
    let generated: BTreeMap<_, _> = project
        .config
        .generated_files
        .iter()
        .map(|(name, path)| {
            (name, syntax::preprocess::normalize_path(&project.workspace_root.join(path)))
        })
        .collect();
    let context = json!({
        "version": 1, "manifest": project.manifest, "workspaceRoot": project.workspace_root,
        "compiler": compiler, "modules": project.config.project_modules,
        "variables": project.config.project_variables, "strictBitvector": project.config.strict_bitvector,
        "target": project.config.target, "includePaths": includes, "generatedFiles": generated,
    });
    let bindings = BTreeMap::from([
        ("project", project.manifest.to_string_lossy().into_owned()),
        ("projectDir", cwd.to_string_lossy().into_owned()),
        ("workspaceRoot", project.workspace_root.to_string_lossy().into_owned()),
        ("sail", compiler.to_owned()),
        ("projectContext", context.to_string()),
    ]);
    let mut args = Vec::new();
    for arg in &task.args {
        if arg == "${projectArgs}" {
            args.extend(project_arguments(project)?);
        } else {
            args.push(scalar(arg, &bindings)?);
        }
    }
    let program = scalar(&task.command, &bindings)?;
    if program.trim().is_empty() {
        return Err("Task command is empty".into());
    }
    let environment = task
        .environment
        .iter()
        .map(|(name, value)| {
            if name.is_empty() || name.contains(['=', '\0']) {
                return Err(format!("Invalid task environment variable name: {name:?}"));
            }
            Ok((name.clone(), scalar(value, &bindings)?))
        })
        .collect::<Result<_, String>>()?;
    Ok(ProcessRunnableArgs { kind: task.kind, program, args, cwd, environment, script: None })
}

/// The compiler's view of the project — manifest, variables, selected
/// roots, strict mode — as `flycheck` passes it, less `--just-check`.
fn project_arguments(project: &FolderProject) -> Result<Vec<String>, String> {
    let (mut selection, _) = crate::flycheck::command_line(
        &crate::flycheck::CheckTarget::Project(project.manifest.clone()),
        &project.config,
        &[],
    )
    .map_err(|reason| {
        format!("{reason}; use a task adapter accepting ${{projectContext}} for this configuration")
    })?;
    let check = selection
        .iter()
        .position(|arg| arg == "--just-check")
        .ok_or("Missing compiler check flag")?;
    selection.remove(check);
    if selection.iter().any(|a| a.contains('\0')) {
        return Err("Project arguments cannot contain NUL".into());
    }
    Ok(selection)
}

/// One entry per `$[test]` function and `main` in `source`, run through the
/// compiler's interpreter: `sail <project args> --is <script>`. See
/// `ide::runnables` for why that is the command.
fn interpreter_entries(
    snap: &GlobalStateSnapshot,
    source: &Url,
    key: &Url,
    project: &FolderProject,
    compiler: &str,
) -> anyhow::Result<Vec<(Runnable, Option<String>)>> {
    let Some(file_id) = snap.analysis.file_id(source) else {
        return Ok(Vec::new());
    };
    let found = snap.analysis.runnables(file_id)?;
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let line_index = snap
        .analysis
        .file_line_index(file_id)
        .unwrap_or_else(|| ide_db::line_index::LineIndex::new(""));
    let cwd =
        project.manifest.parent().ok_or_else(|| anyhow::anyhow!("manifest has no directory"))?;
    let arguments = project_arguments(project);
    Ok(found
        .into_iter()
        .map(|runnable| {
            let (args, problem) = match &arguments {
                Ok(args) => (args.iter().cloned().chain(["--is".to_owned()]).collect(), None),
                Err(reason) => (Vec::new(), Some(reason.clone())),
            };
            let entry = Runnable {
                id: format!("{key}#run-{}", runnable.nav.name),
                label: runnable.label(),
                project: TextDocumentIdentifier::new(
                    Url::from_file_path(&project.manifest).unwrap(),
                ),
                fingerprint: String::new(),
                disabled: None,
                documents: Vec::new(),
                location: Some(lsp_types::Location::new(
                    source.clone(),
                    crate::to_proto::range(&line_index, runnable.nav.full_range),
                )),
                command: RunnableCommand::Process(ProcessRunnableArgs {
                    kind: match runnable.kind {
                        ide::RunnableKind::Test => crate::config::ProjectTaskKind::Test,
                        ide::RunnableKind::Main => crate::config::ProjectTaskKind::Run,
                    },
                    program: compiler.to_owned(),
                    args,
                    cwd: cwd.to_path_buf(),
                    environment: BTreeMap::new(),
                    script: Some(runnable.interpreter_script()),
                }),
            };
            (entry, problem)
        })
        .collect())
}

fn unavailable(snap: &GlobalStateSnapshot, project: &FolderProject, uri: &Url) -> Option<String> {
    if snap.workspace_status.loading {
        return Some("Project reload is pending; retry when loading finishes".into());
    }
    let Some(loaded) = project.manifest_text.as_deref() else {
        return Some("No successfully loaded project manifest".into());
    };
    if snap
        .manifest_texts
        .iter()
        .any(|(open, draft)| crate::from_proto::same_document(open, uri) && draft.data != loaded)
    {
        return Some("Save the manifest and wait for the project to reload".into());
    }
    match std::fs::read_to_string(&project.manifest) {
        Ok(disk) if disk == loaded => None,
        Ok(_) => Some("The manifest on disk has changed; wait for project reload".into()),
        Err(error) => Some(format!("Cannot read project manifest: {error}")),
    }
}

pub(crate) fn discover(snap: &GlobalStateSnapshot, source: &Url) -> anyhow::Result<Vec<Runnable>> {
    let mut out = Vec::new();
    let roots: HashSet<_> = snap.workspace_status.roots.iter().cloned().collect();
    for (key, project) in snap.projects.iter() {
        snap.analysis.with_db(|_| ())?;
        let Ok(uri) = Url::from_file_path(&project.manifest) else { continue };
        if !crate::from_proto::same_document(&uri, source)
            && !project
                .files
                .iter()
                .chain(&project.discovered_files)
                .any(|(url, _)| crate::from_proto::same_document(url, source))
        {
            continue;
        }
        let identifier = TextDocumentIdentifier::new(uri.clone());
        let documents: Vec<_> = project
            .files
            .iter()
            .chain(&project.discovered_files)
            .map(|(url, _)| url.clone())
            .chain([uri.clone()])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let disabled = unavailable(snap, project, &uri);
        let manifest_label = project
            .manifest
            .strip_prefix(&project.workspace_root)
            .unwrap_or(&project.manifest)
            .display();
        let selection = if project.config.project_modules.is_empty() {
            "all modules".to_owned()
        } else {
            project.config.project_modules.join(", ")
        };
        let label = format!("{manifest_label} [{selection}]");
        let check = Runnable {
            id: format!("{key}#check"),
            label: format!("Check {label}"),
            project: identifier.clone(),
            fingerprint: String::new(),
            disabled: disabled.clone(),
            documents: documents.clone(),
            location: None,
            command: RunnableCommand::Check(FlycheckControlParams {
                text_document: Some(identifier.clone()),
            }),
        };
        let config = snap.config_for_uri(
            &Url::from_directory_path(&project.workspace_root)
                .unwrap_or_else(|_| identifier.uri.clone()),
        );
        let mut entries = vec![check];
        for (index, task) in config.runnables.tasks.iter().enumerate() {
            if task.project.as_ref().is_some_and(|filter| {
                syntax::preprocess::normalize_path(&project.workspace_root.join(filter))
                    != syntax::preprocess::normalize_path(&project.manifest)
            }) {
                continue;
            }
            let result = process(project, &config.flycheck.command, task, &roots);
            let (command, problem) = match result {
                Ok(command) => (command, None),
                Err(reason) => (
                    ProcessRunnableArgs {
                        kind: task.kind,
                        program: String::new(),
                        args: Vec::new(),
                        cwd: project.manifest.parent().unwrap().to_path_buf(),
                        environment: BTreeMap::new(),
                        script: None,
                    },
                    Some(reason),
                ),
            };
            entries.push(Runnable {
                id: format!("{key}#task-{index}"),
                label: format!("{} ({label})", task.label),
                project: identifier.clone(),
                fingerprint: String::new(),
                disabled: disabled.clone().or(problem),
                documents: documents.clone(),
                location: None,
                command: RunnableCommand::Process(command),
            });
        }
        for (mut entry, problem) in
            interpreter_entries(snap, source, key, project, &config.flycheck.command)?
        {
            entry.documents = documents.clone();
            entry.disabled = disabled.clone().or(problem);
            entries.push(entry);
        }
        for mut runnable in entries {
            // Where an entry sits moves with every edit above it; what it
            // runs does not, and a lens clicked after such an edit must
            // still run.
            let placed = Runnable { location: None, ..runnable.clone() };
            runnable.fingerprint = crate::code_action_helpers::source_fingerprint(
                &json!({
                    "runnable": placed, "configuration": project.config,
                    "compiler": config.flycheck, "manifest": project.manifest_text,
                })
                .to_string(),
            );
            out.push(runnable);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectTaskKind, ProjectVariableValue, WorkspaceConfig};

    fn fixture() -> (FolderProject, ProjectTaskConfig) {
        let root = std::env::temp_dir().join("sail task $(do-not-execute)");
        let project = FolderProject {
            manifest: root.join("model/model.sail_project"),
            workspace_root: root,
            config: WorkspaceConfig {
                project_variables: BTreeMap::from([(
                    "ARCH".into(),
                    ProjectVariableValue::String("A 64".into()),
                )]),
                project_modules: vec!["Root".into()],
                strict_bitvector: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let task = ProjectTaskConfig {
            label: "Export C".into(),
            kind: ProjectTaskKind::Export,
            command: "${sail}".into(),
            args: vec![
                "-c".into(),
                "${projectArgs}".into(),
                "-o".into(),
                "${projectDir}/output name".into(),
            ],
            environment: BTreeMap::new(),
            project: None,
        };
        (project, task)
    }

    #[test]
    fn project_arguments_preserve_spaces_roots_strict_mode_and_toolchain() {
        let (project, task) = fixture();
        let run = process(&project, "/tool chain/sail", &task, &HashSet::new()).unwrap();
        assert_eq!(run.program, "/tool chain/sail");
        assert_eq!(
            &run.args[..8],
            [
                "-c",
                "--project",
                "model.sail_project",
                "--variable",
                "ARCH=A 64",
                "--strict-bitvector",
                "Root",
                "-o"
            ]
        );
        assert_eq!(run.cwd, project.manifest.parent().unwrap());
        assert!(!run.args.iter().any(|a| a == "--just-check"));
    }

    #[test]
    fn typed_context_preserves_values_the_stock_cli_cannot_represent() {
        let (mut project, mut task) = fixture();
        project
            .config
            .project_variables
            .insert("TYPED".into(), ProjectVariableValue::String("true".into()));
        project.config.target = Some("c".into());
        project.config.include_paths.push("includes".into());
        project.config.generated_files.insert("generated.sail".into(), "out/generated.sail".into());
        assert!(process(&project, "sail", &task, &HashSet::new())
            .unwrap_err()
            .contains("projectContext"));
        task.command = "adapter".into();
        task.args = vec!["${projectContext}".into()];
        let roots = HashSet::from([Url::from_directory_path(&project.workspace_root).unwrap()]);
        let run = process(&project, "chosen-sail", &task, &roots).unwrap();
        let context: serde_json::Value = serde_json::from_str(&run.args[0]).unwrap();
        assert_eq!(context["variables"]["TYPED"], "true");
        assert_eq!(context["compiler"], "chosen-sail");
        assert_eq!(context["modules"], json!(["Root"]));
        assert_eq!(context["target"], "c");
        assert_eq!(context["includePaths"], json!([project.workspace_root.join("includes")]));
        assert_eq!(
            context["generatedFiles"]["generated.sail"],
            json!(project.workspace_root.join("out").join("generated.sail"))
        );
        task.args.clear();
        task.environment.insert("SAIL_LSP_PROJECT_CONTEXT".into(), "${projectContext}".into());
        let run = process(&project, "chosen-sail", &task, &roots).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&run.environment["SAIL_LSP_PROJECT_CONTEXT"])
                .unwrap(),
            context
        );
    }

    #[test]
    fn tasks_must_opt_into_project_context_and_expansion_is_not_recursive() {
        let (project, mut task) = fixture();
        task.args = vec!["-c".into()];
        assert!(process(&project, "sail", &task, &HashSet::new()).is_err());
        let binding = BTreeMap::from([("sail", "${workspaceRoot}/literal $(x)".into())]);
        assert_eq!(scalar("${sail}", &binding).unwrap(), "${workspaceRoot}/literal $(x)");
        assert!(scalar("${projectArgs}", &binding).is_err());
    }
}
