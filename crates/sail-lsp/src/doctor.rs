use crate::install;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

#[allow(clippy::print_stdout)]
pub(crate) fn run(args: &[String]) -> std::result::Result<(), String> {
    install::configure_cli_cancellation()
        .and_then(|()| diagnose(args))
        .and_then(|()| {
            anyhow::ensure!(!install::cancelled(), "Doctor cancelled");
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))
}
#[allow(clippy::print_stdout)]
fn diagnose(args: &[String]) -> Result<()> {
    let mut configured_compiler = None;
    let mut native_stdlib = None;
    let mut options = args.iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--json" | "--help" | "-h" => {}
            "--compiler" => {
                configured_compiler =
                    Some(options.next().context("--compiler needs a path")?.clone())
            }
            "--stdlib" => {
                native_stdlib =
                    Some(PathBuf::from(options.next().context("--stdlib needs a path")?))
            }
            _ => anyhow::bail!(
                "Usage: sail-lsp doctor [--json] [--compiler PATH] [--stdlib SAIL_DIR]"
            ),
        }
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("sail-lsp doctor [--json] [--compiler PATH] [--stdlib SAIL_DIR]\nCheck executable provenance, standard library, SMT backend, PATH copies and optional Sail compiler. Use --compiler to inspect the editor's flycheck.command.");
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let mut checks = Vec::<Value>::new();
    let temp = tempfile::tempdir()?;
    let embedded = crate::sail_stdlib::materialise_stdlib(temp.path())?;
    checks.push(json!({"name":"embeddedStdlib","status":if embedded.join("lib/core/corelib.sail").is_file(){"ok"}else{"error"},"revision":crate::sail_stdlib::SAIL_STDLIB_REVISION.trim()}));
    if let Some(path) = std::env::var_os("SAIL_DIR") {
        let path = PathBuf::from(path);
        checks.push(json!({"name":"stdlibOverride","path":path,"status":if path.join("lib/core/corelib.sail").is_file(){"ok"}else{"error"}}));
    }
    #[cfg(feature = "z3-solver")]
    {
        let solver = z3::Solver::new();
        let n = z3::ast::Int::new_const("sail_lsp_doctor");
        solver.assert(n.eq(1));
        checks.push(json!({"name":"solver","status":if solver.check()==z3::SatResult::Sat{"ok"}else{"error"},"build":crate::version::metadata()["solver"]}));
    }
    #[cfg(not(feature = "z3-solver"))]
    checks.push(
        json!({"name":"solver","status":"warning","message":"This build has no SMT backend"}),
    );
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .filter_map(|dir| {
                    let path = dir.join(install::executable_name());
                    path.is_file().then_some(path)
                })
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths.dedup();
    checks.push(
        json!({"name":"pathCopies","status":if paths.len()>1{"warning"}else{"ok"},"paths":paths}),
    );
    let compiler = configured_compiler.map(PathBuf::from).or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(if cfg!(windows) { "sail.exe" } else { "sail" }))
                .find(|p| p.is_file())
        })
    });
    if let Some(compiler) = compiler {
        let native = native_stdlib
            .or_else(|| std::env::var_os("SAIL_DIR").map(PathBuf::from))
            .unwrap_or(embedded);
        let mut probe = crate::flycheck::toolchain::ProbeCache::default();
        let report = probe.report(
            &compiler.to_string_lossy(),
            &std::env::current_dir()?,
            Some(&native),
            &[],
            |option| {
                let mut command = Command::new(&compiler);
                command.arg(option);
                install::probe_command(command)
                    .map(|output| String::from_utf8_lossy(&output).into_owned())
                    .map_err(|e| e.to_string())
            },
        );
        checks.push(json!({"name":"upstreamSail","status":if report.warnings.is_empty(){"ok"}else{"warning"},"path":compiler,"version":report.version,"pairing":report}));
    } else {
        checks.push(json!({"name":"upstreamSail","status":"optional","message":"Native analysis works without the upstream Sail compiler; Flycheck needs it"}));
    }
    let config = install::default_config_home()?.join("vscode.json");
    if config.is_file() {
        let registration = (|| -> Result<Value> {
            serde_json::from_slice(&std::fs::read(&config)?)
                .context("Invalid managed VS Code registration")
        })();
        match registration {
            Ok(value) => {
                let path = value["path"].as_str().map(PathBuf::from);
                let valid = value["schemaVersion"] == 1
                    && path.as_ref().is_some_and(|p| p.is_absolute() && p.is_file());
                checks.push(json!({"name":"vscodeRegistration","path":config,
                    "status":if valid {"ok"} else {"error"},"value":value}));
            }
            Err(error) => checks.push(json!({"name":"vscodeRegistration","path":config,
                "status":"error","message":error.to_string()})),
        }
    }
    let ok = !checks.iter().any(|c| c["status"] == "error");
    let report = json!({"schemaVersion":1,"ok":ok,"executable":executable,"build":crate::version::metadata(),"checks":checks});
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "sail-lsp {}\nExecutable: {}\nTarget: {}",
            crate::version::version(),
            executable.display(),
            env!("SAIL_LSP_BUILD_TARGET")
        );
        for check in report["checks"].as_array().unwrap() {
            println!("{}: {}", check["name"].as_str().unwrap_or("check"), check);
        }
    }
    anyhow::ensure!(ok, "Doctor found an unusable runtime configuration");
    Ok(())
}
