use std::{
    io::Write,
    process::{Command, Stdio},
};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sail-lsp"))
}

#[test]
fn cli_checks_formats_and_shares_project_style_with_stdin() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("model with spaces");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join(".sailfmt.toml"),
        "style_version=1\nline_width=50\ntab_size=4\ninsert_spaces=true\n",
    )
    .unwrap();
    let path = directory.join("main.sail");
    let original =
        "function f(x) = if x > 0 then first_long_function(x) else second_long_function(x)\n";
    std::fs::write(&path, original).unwrap();
    let check = command().args(["fmt", "--check"]).arg(&path).output().unwrap();
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("Would format"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let result = command().arg("fmt").arg(&directory).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let formatted = std::fs::read_to_string(&path).unwrap();
    assert_ne!(formatted, original);
    assert!(formatted.contains("\n    if"), "{formatted}");
    assert!(command().args(["fmt", "--check"]).arg(&path).status().unwrap().success());
    let mut process = command()
        .args(["fmt", "--explain", "--stdin-filepath"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process.stdin.take().unwrap().write_all(original.as_bytes()).unwrap();
    let result = process.wait_with_output().unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), formatted);
    let report: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(report["status"], "changed");
    assert_eq!(report["configFile"], directory.join(".sailfmt.toml").to_str().unwrap());
}

#[test]
fn cli_validates_all_files_before_writing_and_rejects_bad_configuration() {
    let root = tempfile::tempdir().unwrap();
    let good = root.path().join("a.sail");
    let bad = root.path().join("b.sail");
    let original = "function f() = { let x=1; x }\n";
    std::fs::write(&good, original).unwrap();
    std::fs::write(&bad, "function f() = {\n").unwrap();
    let result = command().arg("fmt").arg(root.path()).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("parse_error"));
    assert_eq!(std::fs::read_to_string(&good).unwrap(), original);
    std::fs::write(root.path().join(".sailfmt.toml"), "style_version=999\n").unwrap();
    let result = command().args(["fmt", "--check"]).arg(&good).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("style_version"));
    assert_eq!(std::fs::read_to_string(&good).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn cli_keeps_symlinks_permissions_and_the_selected_paths_project_style() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let model = root.path().join("model");
    std::fs::create_dir(&model).unwrap();
    let target = root.path().join("real.sail");
    let path = model.join("linked.sail");
    std::fs::write(
        &target,
        "function f(x) = if x > 0 then first_long_function(x) else second_long_function(x)\n",
    )
    .unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &path).unwrap();
    std::fs::write(model.join(".sailfmt.toml"), "line_width=50\ntab_size=8\n").unwrap();
    let result = command().arg("fmt").arg(&path).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(path.is_symlink());
    assert!(std::fs::read_to_string(&target).unwrap().contains("\n        if"));
    assert_eq!(target.metadata().unwrap().permissions().mode() & 0o777, 0o640);
    assert!(command().args(["fmt", "--check"]).arg(&path).status().unwrap().success());
}
