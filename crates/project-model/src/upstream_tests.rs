//! Project semantics as upstream Sail decides them.
//!
//! `CASES` is the portable half: each expectation is what
//! `sail --project <manifest> <roots> --list-files` printed, recorded with
//! Sail 0.20.2 (opam) and 0.20.3 (ac09f8870b9d), which agree on every case
//! not marked `since_0_20_3`. The manifests under `UPSTREAM` are copied from
//! `sail/test/typecheck/project` and `sail/test/project/failure`.
//!
//! `upstream_oracle` is the other half, run on demand: it replays every case
//! and both upstream directories against a real compiler, comparing the file
//! list and each module's `requires` set. It fails, rather than passing
//! vacuously, when the compiler or a corpus is missing.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{ProjectManifest, ProjectOptions, ProjectValue};

struct Case {
    name: &'static str,
    manifest: &'static str,
    variables: &'static [(&'static str, Var)],
    /// Space-separated root modules; empty is `--all-modules`.
    roots: &'static str,
    /// Space-separated file list, or `!` and a word of the rejection.
    expect: &'static str,
    /// Sail 0.20.2 does not parse `implicit`.
    since_0_20_3: bool,
}
#[derive(Clone, Copy)]
enum Var {
    Bool(bool),
    Str(&'static str),
}

const fn case(
    name: &'static str,
    manifest: &'static str,
    roots: &'static str,
    expect: &'static str,
) -> Case {
    Case { name, manifest, variables: &[], roots, expect, since_0_20_3: false }
}

/// Upstream's own project tests, verbatim.
const UPSTREAM: &[(&str, &str)] = &[
    (
        "enum_vis",
        "A {\n  files enum_vis/amod.sail\n}\n\nB {\n  requires A\n  files enum_vis/bmod.sail\n}\n",
    ),
    ("fail_enum_vis", "A {\n  files enum_vis/amod.sail\n}\n\nB {\n  files enum_vis/bmod.sail\n}\n"),
    (
        "fail_global_constraint2",
        "A {\n  files global_constraint2/amod.sail\n}\n\nB {\n  files global_constraint2/bmod.sail\n}\n\nC {\n  requires B\n  files global_constraint2/cmod.sail\n}\n",
    ),
    (
        "global_constraint2",
        "A {\n  files global_constraint2/amod.sail\n}\n\nB {\n  requires A\n  files global_constraint2/bmod.sail\n}\n\nC {\n  requires B\n  files global_constraint2/cmod.sail\n}\n",
    ),
    ("interface", "A {\n  files interface/inst.sail\n}\n"),
    (
        "sizeof_local",
        "P {\n  files sizeof_local/prelude.sail\n}\n\nA {\n  requires P\n  files sizeof_local/a.sail\n}\n\nB {\n  after A\n  requires P\n  files sizeof_local/b.sail\n}\n",
    ),
    (
        "submodules_include",
        "A {\n  ASub {\n    files simple/amod.sail\n  }\n}\n\nB {\n  requires A\n  files simple/bmod.sail\n}\n",
    ),
    ("cyclic", "A {\n  requires B\n}\n\nB {\n  requires A\n}\n"),
    ("default_optional", "A {\n  default\n  B {\n    optional\n  }\n}\n"),
    ("duplicate", "A {\n}\n\nA {\n}"),
    (
        "outer_cyclic_equiv",
        "// This module structure is equivalent to the one in `outer_cyclic`\nA {\n\n}\n\nB {\n  requires A, C\n}\n\nC {\n  requires A, B\n}\n",
    ),
    ("outer_cyclic", "A {\n  B {\n    requires C\n  }\n}\n\nC {\n  requires A\n}\n"),
];
fn upstream(name: &str) -> &'static str {
    UPSTREAM.iter().find(|(n, _)| *n == name).unwrap().1
}

fn cases() -> Vec<Case> {
    let u = upstream;
    vec![
        case("enum_vis", u("enum_vis"), "", "enum_vis/amod.sail enum_vis/bmod.sail"),
        case("enum_vis", u("enum_vis"), "A", "enum_vis/amod.sail"),
        case("enum_vis", u("enum_vis"), "B", "enum_vis/amod.sail enum_vis/bmod.sail"),
        case("fail_enum_vis", u("fail_enum_vis"), "B", "enum_vis/bmod.sail"),
        case(
            "fail_global_constraint2",
            u("fail_global_constraint2"),
            "C",
            "global_constraint2/bmod.sail global_constraint2/cmod.sail",
        ),
        case(
            "global_constraint2",
            u("global_constraint2"),
            "B",
            "global_constraint2/amod.sail global_constraint2/bmod.sail",
        ),
        case(
            "global_constraint2",
            u("global_constraint2"),
            "C",
            "global_constraint2/amod.sail global_constraint2/bmod.sail global_constraint2/cmod.sail",
        ),
        case("interface", u("interface"), "", "interface/inst.sail"),
        case(
            "sizeof_local",
            u("sizeof_local"),
            "",
            "sizeof_local/prelude.sail sizeof_local/a.sail sizeof_local/b.sail",
        ),
        case(
            "sizeof_local",
            u("sizeof_local"),
            "A",
            "sizeof_local/prelude.sail sizeof_local/a.sail",
        ),
        // `after A` orders B after A but does not bring A.
        case(
            "sizeof_local",
            u("sizeof_local"),
            "B",
            "sizeof_local/prelude.sail sizeof_local/b.sail",
        ),
        case(
            "submodules_include",
            u("submodules_include"),
            "",
            "simple/amod.sail simple/bmod.sail",
        ),
        // A parent does not bring its children ...
        case("submodules_include", u("submodules_include"), "A", ""),
        // ... a child brings its parent, and `requires A` means `tree(A)`.
        case("submodules_include", u("submodules_include"), "ASub", "simple/amod.sail"),
        case(
            "submodules_include",
            u("submodules_include"),
            "B",
            "simple/amod.sail simple/bmod.sail",
        ),
        case("cyclic", u("cyclic"), "", "!Cyclic"),
        case("default_optional", u("default_optional"), "", "!reserved"),
        case("duplicate", u("duplicate"), "", "!Duplicate"),
        case("outer_cyclic_equiv", u("outer_cyclic_equiv"), "", "!Cyclic"),
        case("outer_cyclic", u("outer_cyclic"), "", "!Cyclic"),
        // Several roots load in module order, not argument order.
        case(
            "roots",
            "A { files a.sail }\nB { files b.sail }\nC { requires A files c.sail }\n",
            "C A",
            "a.sail c.sail",
        ),
        case(
            "roots",
            "A { files a.sail }\nB { files b.sail }\nC { requires A files c.sail }\n",
            "B A",
            "a.sail b.sail",
        ),
        case(
            "child",
            "P { files p.sail C { files c.sail } }\nQ { requires C files q.sail }\n",
            "C",
            "p.sail c.sail",
        ),
        case(
            "child",
            "P { files p.sail C { files c.sail } }\nQ { requires C files q.sail }\n",
            "Q",
            "p.sail c.sail q.sail",
        ),
        case(
            "order",
            "C { after A files c.sail }\nA { files a.sail }\nB { before A files b.sail }\n",
            "",
            "b.sail a.sail c.sail",
        ),
        case(
            "selector",
            "Main { requires only(P) files m.sail }\nP { files p.sail Child { files c.sail } }\nTree { requires P files x.sail }\n",
            "",
            "p.sail m.sail c.sail x.sail",
        ),
        case(
            "selector",
            "Main { requires only(P) files m.sail }\nP { files p.sail Child { files c.sail } }\nTree { requires P files x.sail }\n",
            "Main",
            "p.sail m.sail",
        ),
        case(
            "selector",
            "Main { requires only(P) files m.sail }\nP { files p.sail Child { files c.sail } }\nTree { requires P files x.sail }\n",
            "Tree",
            "p.sail c.sail x.sail",
        ),
        // A path in two modules is listed twice.
        case(
            "duplicate_file",
            "A { files dup.sail, a.sail }\nB { files dup.sail, b.sail }\n",
            "",
            "dup.sail a.sail dup.sail b.sail",
        ),
        case(
            "escape",
            "M { files \"a\\x41.sail\", \"b\\066.sail\", \"c\\\\d.sail\", \"\\x61.sail\" }\n",
            "",
            "aA.sail bB.sail c\\d.sail a.sail",
        ),
        case("escape", "M { files \"a\\q.sail\" }\n", "", "!escape"),
        // Reserved words are rejected as bare words, not as module headers or file names.
        case("reserved", "variable optional = true\nM { files a.sail }\n", "", "!reserved"),
        case("reserved", "default { files a.sail }\n", "", "a.sail"),
        case("reserved", "M { files default.sail }\n", "", "default.sail"),
        Case {
            since_0_20_3: true,
            ..case(
                "implicit",
                "L { files lib/l.sail }\nimplicit only(L)\nP { files p.sail C { files c.sail } }\n",
                "",
                "lib/l.sail p.sail c.sail",
            )
        },
        Case {
            since_0_20_3: true,
            ..case(
                "implicit",
                "L { files lib/l.sail }\nimplicit only(L)\nP { files p.sail C { files c.sail } }\n",
                "P",
                "lib/l.sail p.sail",
            )
        },
        case("variables", VARIABLES, "", "b.sail m.sail"),
        Case {
            variables: &[("X", Var::Bool(true))],
            ..case("variables", VARIABLES, "", "a.sail m.sail")
        },
        Case {
            variables: &[("ARCH", Var::Str("A32"))],
            ..case("variables", VARIABLES, "", "b.sail c.sail")
        },
        // A string is not a condition, even one spelled "true".
        Case {
            variables: &[("X", Var::Str("on"))],
            ..case("variables", VARIABLES, "", "!boolean")
        },
    ]
}
const VARIABLES: &str = "variable X = false\nvariable ARCH = A64\nM { files if $X then a.sail else b.sail, if $ARCH == A32 then c.sail else m.sail }\n";

fn options(case: &Case) -> ProjectOptions {
    ProjectOptions {
        variables: case
            .variables
            .iter()
            .map(|(name, value)| {
                let value = match *value {
                    Var::Bool(b) => ProjectValue::Bool(b),
                    Var::Str(s) => ProjectValue::String(s.into()),
                };
                (name.to_string(), value)
            })
            .collect(),
        roots: case.roots.split_whitespace().map(String::from).collect(),
        ..Default::default()
    }
}
fn listed(manifest: &str, options: &ProjectOptions) -> Result<Vec<String>, String> {
    let project =
        ProjectManifest::parse(manifest).and_then(|m| m.resolve(options)).map_err(|e| e.message)?;
    Ok(project.file_occurrences().map(|(_, p)| p.to_string_lossy().into_owned()).collect())
}

#[test]
fn file_lists_match_recorded_upstream_results() {
    let mut failures = Vec::new();
    for case in cases() {
        let actual = listed(case.manifest, &options(&case));
        let ok = match (case.expect.strip_prefix('!'), &actual) {
            (Some(word), Err(message)) => message.contains(word),
            (None, Ok(files)) => *files == case.expect.split_whitespace().collect::<Vec<_>>(),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "{} [{}]: expected {}, got {actual:?}",
                case.name, case.roots, case.expect
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---- the on-demand oracle ------------------------------------------------

struct Oracle {
    sail: PathBuf,
    version: String,
}
impl Oracle {
    fn from_env() -> Oracle {
        let sail = std::env::var_os("SAIL_ORACLE_BIN")
            .map_or_else(|| PathBuf::from("sail"), PathBuf::from);
        let output = std::process::Command::new(&sail)
            .arg("--version")
            .output()
            .unwrap_or_else(|e| panic!("cannot run {} (set SAIL_ORACLE_BIN): {e}", sail.display()));
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        eprintln!("oracle: {} = {version}", sail.display());
        Oracle { sail, version }
    }
    fn supports_implicit(&self) -> bool {
        !self.version.starts_with("Sail 0.20.2")
    }
    /// `--list-files` for these roots, or the error upstream printed.
    fn list(
        &self,
        manifest: &Path,
        variables: &[String],
        roots: &[String],
    ) -> Result<Vec<String>, String> {
        // Relative to the manifest, so upstream prints paths as the manifest spells them.
        let mut cmd = std::process::Command::new(&self.sail);
        cmd.arg("--project").arg(manifest.file_name().unwrap()).arg("--list-files");
        for v in variables {
            cmd.arg("--variable").arg(v);
        }
        if roots.is_empty() {
            cmd.arg("--all-modules");
        } else {
            cmd.args(roots);
        }
        let output = cmd.current_dir(manifest.parent().unwrap()).output().unwrap();
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).split_whitespace().map(String::from).collect())
    }
    /// Each module's `requires` set, from the project dependency graph dump.
    fn requires(&self, manifest: &Path) -> BTreeMap<String, BTreeSet<String>> {
        let output = std::process::Command::new(&self.sail)
            .arg("--project")
            .arg(manifest.file_name().unwrap())
            .args(["--all-modules", "--just-parse-project", "--ddump-project-depgraph", "stdout"])
            .arg("--ddump-project-depgraph-skip-deps")
            .current_dir(manifest.parent().unwrap())
            .output()
            .unwrap();
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            // `"A" -> "B" [..style=solid]` is drawn reversed: B requires A.
            let names: Vec<_> = line.split('"').skip(1).step_by(2).collect();
            if let [required, by] = names[..] {
                if line.contains("->") {
                    out.entry(by.to_owned()).or_default().insert(required.to_owned());
                }
            }
        }
        out
    }
}

fn sibling(env: &str, relative: &str) -> PathBuf {
    let path = std::env::var_os(env).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(relative),
        PathBuf::from,
    );
    path.canonicalize().unwrap_or_else(|e| panic!("{} (set {env}): {e}", path.display()))
}

fn lsp_requires(manifest: &str) -> BTreeMap<String, BTreeSet<String>> {
    let project =
        ProjectManifest::parse(manifest).unwrap().resolve(&ProjectOptions::default()).unwrap();
    let name = |id: usize| project.modules[id].name.clone();
    (0..project.modules.len())
        .filter(|&id| !project.modules[id].requires.is_empty())
        .map(|id| (name(id), project.modules[id].requires.iter().map(|&r| name(r)).collect()))
        .collect()
}

/// Compare one manifest on disk: file lists for all modules and each single
/// root, and the `requires` graph. Returns what disagreed.
fn compare_manifest(
    oracle: &Oracle,
    path: &Path,
    variables: &[(String, ProjectValue)],
) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    let cli: Vec<_> = variables
        .iter()
        .map(|(n, v)| match v {
            ProjectValue::Bool(b) => format!("{n}={b}"),
            ProjectValue::String(s) => format!("{n}={s}"),
            _ => unreachable!(),
        })
        .collect();
    let mut options =
        ProjectOptions { variables: variables.iter().cloned().collect(), ..Default::default() };
    let mut problems = Vec::new();
    let label = path.display();
    let all = listed(&text, &options);
    match (&all, oracle.list(path, &cli, &[])) {
        (Err(_), Err(_)) => return problems,
        (Ok(ours), Ok(theirs)) if *ours == theirs => {}
        (ours, theirs) => {
            problems.push(format!(
                "{label} {cli:?} all modules: sail-lsp {ours:?}, upstream {theirs:?}"
            ));
            return problems;
        }
    }
    let theirs = oracle.requires(path);
    let ours = lsp_requires(&text);
    let theirs: BTreeMap<_, _> = theirs
        .into_iter()
        .filter_map(|(m, rs)| {
            // Upstream adds its embedded corelib module to every project.
            let rs: BTreeSet<_> = rs.into_iter().filter(|r| r != "corelib").collect();
            (m != "corelib" && !rs.is_empty()).then_some((m, rs))
        })
        .collect();
    if ours != theirs {
        problems.push(format!("{label}: requires sail-lsp {ours:?}, upstream {theirs:?}"));
    }
    let modules = ProjectManifest::parse(&text).unwrap().resolve(&options).unwrap().modules;
    for module in modules {
        options.roots = vec![module.name.clone()];
        let ours = listed(&text, &options);
        let theirs = oracle.list(path, &cli, &options.roots);
        if ours != theirs {
            problems.push(format!(
                "{label} {cli:?} root {}: sail-lsp {ours:?}, upstream {theirs:?}",
                module.name
            ));
        }
    }
    problems
}

#[test]
#[ignore = "runs a Sail compiler (SAIL_ORACLE_BIN, default `sail`) over the sibling sail and sail-riscv checkouts (SAIL_ORACLE_SRC, SAIL_ORACLE_RISCV)"]
fn upstream_oracle() {
    let oracle = Oracle::from_env();
    let mut problems = Vec::new();
    let (mut compared, mut skipped) = (0, 0);

    // The recorded table must still be what upstream says.
    let scratch =
        std::env::temp_dir().join(format!("sail-lsp-project-oracle-{}", std::process::id()));
    for (i, case) in cases().iter().enumerate() {
        if case.since_0_20_3 && !oracle.supports_implicit() {
            eprintln!("skipped {} [{}]: needs Sail 0.20.3", case.name, case.roots);
            skipped += 1;
            continue;
        }
        let dir = scratch.join(i.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(format!("{}.sail_project", case.name));
        std::fs::write(&manifest, case.manifest).unwrap();
        // Upstream reads every module's files, selected or not.
        let every_module = ProjectOptions { roots: Vec::new(), ..options(case) };
        if let Ok(project) =
            ProjectManifest::parse(case.manifest).and_then(|m| m.resolve(&every_module))
        {
            for file in project.modules.iter().flat_map(|m| &m.files) {
                std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
                std::fs::write(dir.join(file), "").unwrap();
            }
        }
        let variables: Vec<_> = case
            .variables
            .iter()
            .map(|(n, v)| match v {
                Var::Bool(b) => format!("{n}={b}"),
                Var::Str(s) => format!("{n}={s}"),
            })
            .collect();
        let roots: Vec<_> = case.roots.split_whitespace().map(String::from).collect();
        let upstream = oracle.list(&manifest, &variables, &roots);
        let agrees = match (case.expect.strip_prefix('!'), &upstream) {
            (Some(_), Err(_)) => true,
            (None, Ok(files)) => *files == case.expect.split_whitespace().collect::<Vec<_>>(),
            _ => false,
        };
        if !agrees {
            problems.push(format!(
                "recorded {} [{}] = {}, upstream {upstream:?}",
                case.name, case.roots, case.expect
            ));
        }
        compared += 1;
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let src = sibling("SAIL_ORACLE_SRC", "sail");
    for dir in ["test/typecheck/project", "test/project/failure"] {
        let mut manifests: Vec<_> = std::fs::read_dir(src.join(dir))
            .unwrap_or_else(|e| panic!("{}: {e}", src.join(dir).display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "sail_project"))
            .collect();
        manifests.sort();
        assert!(!manifests.is_empty(), "no manifests under {}", src.join(dir).display());
        for path in manifests {
            problems.extend(compare_manifest(&oracle, &path, &[]));
            compared += 1;
        }
    }

    let riscv = sibling("SAIL_ORACLE_RISCV", "sail-riscv").join("model/riscv.sail_project");
    for rmem in [false, true] {
        let variables = [("RMEM".to_owned(), ProjectValue::Bool(rmem))];
        let text =
            std::fs::read_to_string(&riscv).unwrap_or_else(|e| panic!("{}: {e}", riscv.display()));
        let options =
            ProjectOptions { variables: variables.iter().cloned().collect(), ..Default::default() };
        let ours = listed(&text, &options).unwrap();
        let theirs = oracle.list(&riscv, &[format!("RMEM={rmem}")], &[]);
        if Ok(&ours) != theirs.as_ref() {
            problems.push(format!(
                "riscv RMEM={rmem}: sail-lsp {} files, upstream {theirs:?}",
                ours.len()
            ));
        }
        compared += 1;
    }

    eprintln!("oracle: {compared} compared, {skipped} skipped, {} disagreements", problems.len());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
