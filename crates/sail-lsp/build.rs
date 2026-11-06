// Cargo build scripts communicate through stdout.
#![allow(clippy::print_stdout)]

use std::{path::PathBuf, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rustc-env=SAIL_LSP_BUILD_TARGET={}", std::env::var("TARGET").unwrap());
    // Git is optional for packaged source archives. Follow the actual git dir
    // and ref as well as HEAD, including linked worktrees and packed refs.
    println!("cargo:rerun-if-env-changed=SAIL_LSP_BUILD_REVISION");
    // Cargo may reuse this build-script executable across worktrees sharing
    // a target directory. Resolve the current package from the runtime env.
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    // An archive unpacked inside another repository must not claim that
    // unrelated repository's revision.
    let is_checkout = git(&["rev-parse", "--show-toplevel"])
        .and_then(|path| PathBuf::from(path).canonicalize().ok())
        .zip(root.canonicalize().ok())
        .is_some_and(|(git_root, root)| git_root == root);
    if is_checkout {
        for name in ["HEAD", "index", "packed-refs"] {
            if let Some(path) = git(&["rev-parse", "--git-path", name]) {
                println!("cargo:rerun-if-changed={path}");
            }
        }
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
            if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
    for path in ["crates", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let revision = std::env::var("SAIL_LSP_BUILD_REVISION")
        .ok()
        .or_else(|| {
            if !is_checkout {
                return None;
            }
            let mut revision = git(&["rev-parse", "HEAD"])?;
            if git(&["status", "--porcelain", "--untracked-files=normal"])
                .is_some_and(|status| !status.is_empty())
            {
                revision.push_str("-dirty");
            }
            Some(revision)
        })
        .unwrap_or_else(|| "unknown (source archive)".into());
    println!("cargo:rustc-env=SAIL_LSP_BUILD_REVISION={revision}");
}
