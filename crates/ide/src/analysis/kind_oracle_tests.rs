//! The same kind cases run through IDE diagnostics and an optional live Sail.
use super::hierarchy::tests::{setup, snapshot};
use super::*;

fn cases() -> Vec<(&'static str, &'static str, bool)> {
    let mixed = "type Mixed('p : Bool, 'n) = (bool('p), vector('n, dec, int))\n";
    let shadow = "type Shadow('n) = ('n, {'n, 'n >= 0. vector('n, dec, int)})\n";
    let nested = "type Wrap('p : Bool, 'n : Int, 'a : Type) = (bool('p), vector('n, dec, 'a))\ntype Outer('p : Bool, 'n : Int, 'a : Type) = list(Wrap('p, 'n, 'a))\n";
    vec![
        (mixed, "val use : Mixed(true, 8) -> unit\n", true),
        (mixed, "val use : Mixed(8, 8) -> unit\n", false),
        (mixed, "val use : Mixed(true, bool) -> unit\n", false),
        (shadow, "val use : Shadow(bool) -> unit\n", true),
        (shadow, "val use : Shadow(8) -> unit\n", false),
        ("type Id('a) = 'a\n", "", false),
        ("type Id('a : Type) = 'a\n", "val use : Id(bool) -> unit\n", true),
        (nested, "val use : Outer(true, 8, int) -> unit\n", true),
        (nested, "val use : Outer(8, true, int) -> unit\n", false),
        (nested, "val use : Outer(true, int, int) -> unit\n", false),
        (nested, "val use : Outer(true, 8, false) -> unit\n", false),
    ]
}

#[test]
fn kind_oracle_cases_match_ide_in_single_and_multiple_files() {
    for (definitions, usage, accepted) in cases() {
        for split in [false, true] {
            let combined = format!("{definitions}{usage}");
            let files = if split {
                vec![("defs.sail", definitions), ("main.sail", usage)]
            } else {
                vec![("main.sail", combined.as_str())]
            };
            let order: Vec<_> = (0..files.len()).collect();
            let (host, map, _) = setup(&files, &order, vec![]);
            let analysis = snapshot(&host, &map);
            let diagnostics: Vec<_> = order
                .iter()
                .flat_map(|id| {
                    analysis
                        .file_diagnostics(FileId::from_raw(*id as u32), &Default::default())
                        .unwrap()
                })
                .filter(|d| d.severity == ide_diagnostics::Severity::Error)
                .collect();
            assert_eq!(
                diagnostics.is_empty(),
                accepted,
                "split={split}\n{combined}\n{diagnostics:?}"
            );
            if !accepted {
                assert!(
                    diagnostics.iter().any(|d| matches!(
                        &d.code,
                        hir::diagnostics::DiagnosticCode::SailError("kind-mismatch")
                    )),
                    "expected kind rejection: {combined}\n{diagnostics:?}"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires SAIL_ORACLE_BIN; missing compiler is an error"]
fn upstream_kind_oracle() {
    let compiler = std::env::var_os("SAIL_ORACLE_BIN")
        .expect("set SAIL_ORACLE_BIN to the upstream Sail executable");
    let version = std::process::Command::new(&compiler)
        .arg("--version")
        .output()
        .expect("start upstream Sail");
    assert!(version.status.success(), "upstream --version failed");
    // Retained by CI so expected verdicts always have compiler provenance.
    std::io::Write::write_all(&mut std::io::stdout(), &version.stdout).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (index, (definitions, usage, accepted)) in cases().into_iter().enumerate() {
        let path = directory.path().join(format!("kind_case_{index}.sail"));
        let source = format!("{definitions}{usage}");
        std::fs::write(&path, &source).unwrap();
        let output = std::process::Command::new(&compiler)
            .arg("--no-memo-z3")
            .arg("--just-check")
            .arg(&path)
            .output()
            .expect("run upstream kind oracle");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), accepted, "case {index}: {source}\n{stderr}");
        if !accepted {
            assert!(
                stderr.to_lowercase().contains("kind"),
                "wrong rejection reason: {source}\n{stderr}"
            );
        }
    }
}
