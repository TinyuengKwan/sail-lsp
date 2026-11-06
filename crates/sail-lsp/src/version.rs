/// Shared by the CLI, initialize response and analyzer status.
pub(crate) fn version() -> String {
    format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("SAIL_LSP_BUILD_REVISION"))
}

#[allow(clippy::print_stdout)]
pub(crate) fn print_version() {
    println!("sail-lsp {}", version());
}

/// Machine-readable provenance shared by archive packaging and `doctor`.
pub(crate) fn metadata() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion":1,"version":env!("CARGO_PKG_VERSION"),"crtStatic":cfg!(target_feature="crt-static"),
        "revision":env!("SAIL_LSP_BUILD_REVISION"),"target":env!("SAIL_LSP_BUILD_TARGET"),
        "stdlibRevision":crate::sail_stdlib::SAIL_STDLIB_REVISION.trim(),
        "solver":if cfg!(feature="z3-solver") {if cfg!(feature="vendored") {"z3-vendored"} else {"z3-system"}} else {"none"},
    })
}
