//! Upstream Sail's own diagnostics, beside the server's.
//!
//! rust-analyzer runs `cargo check` and shows what the compiler says next to
//! what its own analysis says, because the two are not the same thing and
//! the IDE's is the weaker one. This server had no equivalent — the header
//! of `diagnostics.rs` recorded it as a "Sail simplification" — while every
//! probe in this repo uses `sail --just-check` as its oracle precisely
//! because it decides what the server cannot.
//!
//! What it closes is not hypothetical. `int` where `range(0, 3)` is expected
//! is a false negative this checker cannot fix without carrying every value's
//! bounds (see the reverted directional-coercion experiment), and the five
//! mutants `mutation_check` still misses are the same shape. Upstream proves
//! them.
//!
//! Off by default: it shells out to a binary that may not be installed, and
//! a language server that spawns processes nobody asked for is a surprise.

use std::path::{Path, PathBuf};
use std::process::Command;

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// One run's findings, by the file they belong to.
pub(crate) type FlycheckResult = Vec<(PathBuf, Diagnostic)>;

/// What one run checks. Each loaded project is its own run, as each Cargo
/// workspace is rust-analyzer's: two projects' files in one `sail` call
/// would be checked as one program.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CheckTarget {
    /// A loaded manifest, checked as the server loaded it.
    Project(PathBuf),
    /// A file no loaded project lists, checked alone.
    File(PathBuf),
}

/// The arguments and working directory that check `target`.
///
/// A project gets the manifest, variables and root modules the server
/// resolved it with, so upstream checks the program the editor shows. The
/// CLI reads a bare `true`/`false` as a boolean, so a string variable spelled
/// that way cannot be passed through; `projectVariables` documents this.
pub(crate) fn command_line(
    target: &CheckTarget,
    workspace: &crate::config::WorkspaceConfig,
    extra_args: &[String],
) -> Result<(Vec<String>, PathBuf), String> {
    // The compiler trims assignments and coerces boolean-looking strings.
    // Never silently check a different configuration from the IDE.
    if workspace.target.is_some() {
        return Err("workspace.target: Sail --just-check uses the default target".into());
    }
    if !workspace.include_paths.is_empty() {
        return Err(
            "workspace.includePaths: Sail has no equivalent include search path option".into()
        );
    }
    if matches!(target, CheckTarget::Project(_)) {
        if !workspace.generated_files.is_empty() {
            return Err("workspace.generatedFiles cannot be supplied through Sail's CLI".into());
        }
        for (name, value) in &workspace.project_variables {
            if name.trim() != name || name.contains('=') {
                return Err(format!(
                    "project variable name {name:?} cannot round-trip through Sail's CLI"
                ));
            }
            if let crate::config::ProjectVariableValue::String(value) = value {
                if value.trim() != value || matches!(value.as_str(), "true" | "false") {
                    return Err(format!("project variable {name}: string {value:?} cannot round-trip through Sail's CLI"));
                }
            }
        }
    }
    let mut extra_args = extra_args.to_vec();
    if workspace.strict_bitvector {
        extra_args.insert(0, "--strict-bitvector".into());
    }
    let (path, mut args) = match target {
        CheckTarget::Project(manifest) => (manifest, vec!["--project".to_owned()]),
        CheckTarget::File(file) => (file, Vec::new()),
    };
    // Relative to the directory it runs in, so upstream prints paths
    // relative to it and `parse` resolves them against the same place.
    let cwd = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    match target {
        CheckTarget::Project(_) => {
            args.push(name);
            args.push("--just-check".to_owned());
            for (variable, value) in &workspace.project_variables {
                let value = match value {
                    crate::config::ProjectVariableValue::Bool(b) => b.to_string(),
                    crate::config::ProjectVariableValue::String(s) => s.clone(),
                };
                args.extend(["--variable".to_owned(), format!("{variable}={value}")]);
            }
            args.extend(extra_args.iter().cloned());
            if workspace.project_modules.is_empty() {
                args.push("--all-modules".to_owned());
            } else {
                args.extend(workspace.project_modules.iter().cloned());
            }
        }
        CheckTarget::File(_) => {
            args.push("--just-check".to_owned());
            args.extend(extra_args.iter().cloned());
            args.push(name);
        }
    }
    Ok((args, cwd))
}

/// Run `command` with these arguments and translate what it says.
///
/// `cwd` is where relative paths in the output are resolved from — upstream
/// prints the path it was given.
pub(crate) fn run(command: &str, args: &[String], cwd: &Path) -> FlycheckResult {
    let Ok(output) = Command::new(command).args(args).current_dir(cwd).output() else {
        // No `sail` on PATH is not an error to report in the editor; the
        // feature is opt-in and the user chose the command.
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stderr);
    parse(&text, cwd)
}

/// Upstream's error format.
///
/// ```text
/// Type error:
/// a.sail:4.35-41:
/// 4 |function branch x = if x < 32 then x + 32 else x - 32
///   |                                   ^----^ (operator +)
///   | int(('ex179 + 32)) is not a subtype of range(0, 62)
/// ```
///
/// The location line is the anchor: a header before it says what kind of
/// error it is, and the `|`-prefixed lines after it say why. Source echoes
/// (`4 |...`) and the caret ruler are not the message.
pub(crate) fn parse(text: &str, cwd: &Path) -> FlycheckResult {
    let lines: Vec<String> = text.lines().map(strip_ansi).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((path, range)) = parse_location(&lines[i]) else {
            i += 1;
            continue;
        };
        // The header is the nearest preceding non-empty line; it names the
        // kind ("Type error:", "Syntax error:").
        let header = lines[..i]
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().trim_end_matches(':').to_string())
            .unwrap_or_else(|| "error".to_string());
        let mut message = Vec::new();
        let mut j = i + 1;
        while j < lines.len() {
            let line = lines[j].trim_start();
            // A following location line starts the next diagnostic.
            if parse_location(&lines[j]).is_some() {
                break;
            }
            match line.strip_prefix('|') {
                Some(rest) => {
                    let rest = rest.trim();
                    // What is *not* the message: the caret ruler, an echo of
                    // the source, a nested note's own location line, and a
                    // deeper nesting's prefix.
                    let is_ruler =
                        !rest.is_empty() && rest.chars().all(|c| matches!(c, '^' | '-' | ' '));
                    let is_echo = rest.split_once('|').is_some_and(|(n, _)| {
                        !n.is_empty() && n.trim().chars().all(|c| c.is_ascii_digit())
                    });
                    let is_nested_location = looks_like_location(rest);
                    if !rest.is_empty()
                        && !is_ruler
                        && !is_echo
                        && !is_nested_location
                        && !rest.starts_with('|')
                    {
                        message.push(rest.to_string());
                    }
                }
                // A source echo is `4 |...`; anything else ends the block.
                None if line.split_once('|').is_some_and(|(n, _)| {
                    !n.is_empty() && n.trim().chars().all(|c| c.is_ascii_digit())
                }) => {}
                None if line.is_empty() => {}
                None => break,
            }
            j += 1;
        }
        let body = if message.is_empty() {
            header.clone()
        } else {
            // Two lines is enough to say what happened; upstream's nested
            // overload listings run to dozens.
            message.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
        };
        out.push((
            resolve(&path, cwd),
            Diagnostic {
                range,
                severity: Some(if header.to_ascii_lowercase().contains("warning") {
                    DiagnosticSeverity::WARNING
                } else {
                    DiagnosticSeverity::ERROR
                }),
                source: Some("sail".to_string()),
                message: body,
                ..Default::default()
            },
        ));
        i = j.max(i + 1);
    }
    out
}

fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        cwd.join(p)
    }
}

/// `path:line.col-col:` or `path:line.col-line.col:`, 1-based lines and
/// 0-based columns — which is what LSP wants for the column and one less
/// for the line.
fn parse_location(line: &str) -> Option<(String, Range)> {
    let line = line.trim();
    // Upstream nests notes under a diagnostic, each with its own location
    // line behind a `|`: "derived from here", "bound here". Those belong to
    // the diagnostic above them, and reading them as new ones turned one
    // error into three, two of them at a path called `| a.sail`.
    if line.starts_with('|') {
        return None;
    }
    let line = line.strip_suffix(':')?;
    let (path, span) = line.rsplit_once(':')?;
    if path.is_empty() {
        return None;
    }
    let (start, end) = span.split_once('-')?;
    let (l1, c1) = start.split_once('.')?;
    let l1: u32 = l1.trim().parse().ok()?;
    let c1: u32 = c1.trim().parse().ok()?;
    let (l2, c2) = match end.split_once('.') {
        Some((l, c)) => (l.trim().parse().ok()?, c.trim().parse().ok()?),
        None => (l1, end.trim().parse().ok()?),
    };
    Some((
        path.to_string(),
        Range {
            start: Position { line: l1.saturating_sub(1), character: c1 },
            end: Position { line: l2.saturating_sub(1), character: c2 },
        },
    ))
}

/// `a.sail:3.17-28:` — the shape of a location, wherever it appears.
fn looks_like_location(text: &str) -> bool {
    let Some(rest) = text.strip_suffix(':') else { return false };
    let Some((path, span)) = rest.rsplit_once(':') else { return false };
    !path.is_empty()
        && span.contains('.')
        && span.contains('-')
        && span.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-')
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{1b}[93mType error\u{1b}[0m:\n\
        \u{1b}[96ma.sail\u{1b}[0m:4.35-41:\n\
        4\u{1b}[96m |\u{1b}[0mfunction branch x = if x < 32 then x + 32 else x - 32\n\
        \u{1b}[91m |\u{1b}[0m                                   \u{1b}[91m^----^\u{1b}[0m\n\
        \u{1b}[91m |\u{1b}[0m int(('ex179 + 32)) is not a subtype of range(0, 62)\n";

    #[test]
    fn a_location_line_anchors_a_diagnostic() {
        let found = parse(SAMPLE, Path::new("/w"));
        assert_eq!(found.len(), 1, "{found:?}");
        let (path, diag) = &found[0];
        assert_eq!(path, Path::new("/w/a.sail"));
        // Line 4 is LSP line 3; the columns pass through.
        assert_eq!(diag.range.start, Position { line: 3, character: 35 });
        assert_eq!(diag.range.end, Position { line: 3, character: 41 });
        assert_eq!(diag.source.as_deref(), Some("sail"));
        assert!(
            diag.message.contains("is not a subtype of range(0, 62)"),
            "the ruler and the source echo are not the message: {}",
            diag.message
        );
    }

    /// Two errors in one run, and a span that crosses lines.
    #[test]
    fn each_location_starts_a_new_diagnostic() {
        let text = "Type error:\n\
            b.sail:2.0-3.4:\n\
             | first\n\
            Type error:\n\
            b.sail:9.1-2:\n\
             | second\n";
        let found = parse(text, Path::new("/w"));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].1.range.end, Position { line: 2, character: 4 });
        assert_eq!(found[0].1.message, "first");
        assert_eq!(found[1].1.message, "second");
    }

    /// A project is checked as the server loaded it: its manifest, its
    /// variables, its roots, from its own directory.
    #[test]
    fn a_project_is_checked_with_its_configuration() {
        use crate::config::{ProjectVariableValue, WorkspaceConfig};
        let target = CheckTarget::Project("/w/model/riscv.sail_project".into());
        let mut workspace = WorkspaceConfig::default();
        let (args, cwd) = command_line(&target, &workspace, &["--strict-bv".into()]).unwrap();
        assert_eq!(cwd, Path::new("/w/model"));
        assert_eq!(
            args,
            ["--project", "riscv.sail_project", "--just-check", "--strict-bv", "--all-modules"]
        );

        workspace.project_variables.insert("RMEM".into(), ProjectVariableValue::Bool(true));
        workspace
            .project_variables
            .insert("ARCH".into(), ProjectVariableValue::String("RV32".into()));
        workspace.project_modules = vec!["Main".into(), "Tests".into()];
        let (args, _) = command_line(&target, &workspace, &[]).unwrap();
        assert_eq!(
            args,
            [
                "--project",
                "riscv.sail_project",
                "--just-check",
                "--variable",
                "ARCH=RV32",
                "--variable",
                "RMEM=true",
                "Main",
                "Tests"
            ]
        );

        let (args, cwd) =
            command_line(&CheckTarget::File("/w/x/a.sail".into()), &workspace, &[]).unwrap();
        assert_eq!(cwd, Path::new("/w/x"));
        assert_eq!(
            args,
            ["--just-check", "a.sail"],
            "a file outside every project gets none of it"
        );
    }

    #[test]
    fn invocation_preserves_strict_mode_and_rejects_lossy_configuration() {
        use crate::config::{ProjectVariableValue, WorkspaceConfig};
        let project = CheckTarget::Project("/w/sail.proj".into());
        let file = CheckTarget::File("/w/a.sail".into());
        let strict = WorkspaceConfig { strict_bitvector: true, ..Default::default() };
        for target in [&project, &file] {
            assert!(command_line(target, &strict, &[])
                .unwrap()
                .0
                .contains(&"--strict-bitvector".into()));
        }
        for value in ["true", "false", " true", "value "] {
            let mut config = WorkspaceConfig::default();
            config.project_variables.insert("V".into(), ProjectVariableValue::String(value.into()));
            assert!(command_line(&project, &config, &[]).unwrap_err().contains("variable V"));
            assert!(command_line(&file, &config, &[]).is_ok());
        }
        for key in ["target", "includePaths", "generatedFiles"] {
            let mut config = WorkspaceConfig::default();
            match key {
                "target" => config.target = Some("c".into()),
                "includePaths" => config.include_paths.push("includes".into()),
                "generatedFiles" => {
                    config.generated_files.insert("g.sail".into(), "build/g.sail".into());
                }
                _ => unreachable!(),
            }
            assert!(command_line(&project, &config, &[]).unwrap_err().contains(key));
        }
    }

    /// Output with no location line yields nothing rather than a diagnostic
    /// at 0:0 — a checker that cannot place a finding must not invent a
    /// place for it.
    #[test]
    fn output_without_a_location_is_not_a_diagnostic() {
        assert!(parse("sail: command not found\n", Path::new("/w")).is_empty());
        assert!(parse("", Path::new("/w")).is_empty());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Against the real binary. `#[ignore]` because it needs `sail` on PATH.
    #[test]
    #[ignore]
    fn upstream_says_what_we_read() {
        let dir = std::env::temp_dir().join("sail-lsp-flycheck-live");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.sail");
        std::fs::write(
            &file,
            "default Order dec\nval g : unit -> int\nval f : unit -> range(0, 3)\nfunction f() = g()\n",
        )
        .unwrap();
        let (args, cwd) =
            command_line(&CheckTarget::File(file.clone()), &Default::default(), &[]).unwrap();
        let found = run("sail", &args, &cwd);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, file);
        assert!(found[0].1.message.contains("not a subtype"), "{}", found[0].1.message);
        assert_eq!(found[0].1.range.start.line, 3);
    }

    /// A file that only makes sense in its project is clean when checked as
    /// the project, the variable choosing the branch that defines `x`; its
    /// error is reported at the file the project names.
    #[test]
    #[ignore]
    fn a_project_is_checked_as_a_project() {
        let dir = std::env::temp_dir().join("sail-lsp-flycheck-project");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("p.sail_project"),
            "variable WIDE = false\nA { files if $WIDE then src/wide.sail else src/narrow.sail }\nB { requires A files src/b.sail }\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/narrow.sail"), "default Order dec\nlet x : bool = true\n")
            .unwrap();
        std::fs::write(dir.join("src/wide.sail"), "default Order dec\nlet x : unit = ()\n")
            .unwrap();
        std::fs::write(dir.join("src/b.sail"), "let y : bool = x\n").unwrap();
        let target = CheckTarget::Project(dir.join("p.sail_project"));
        let mut workspace = crate::config::WorkspaceConfig::default();
        let (args, cwd) = command_line(&target, &workspace, &[]).unwrap();
        assert_eq!(run("sail", &args, &cwd), Vec::new());

        workspace
            .project_variables
            .insert("WIDE".into(), crate::config::ProjectVariableValue::Bool(true));
        let (args, cwd) = command_line(&target, &workspace, &[]).unwrap();
        let found = run("sail", &args, &cwd);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, dir.join("src/b.sail"));
        assert_eq!(found[0].1.range.start.line, 0);

        let (args, cwd) =
            command_line(&CheckTarget::File(dir.join("src/b.sail")), &workspace, &[]).unwrap();
        assert!(!run("sail", &args, &cwd).is_empty(), "alone, b.sail does not know x");
    }
}
