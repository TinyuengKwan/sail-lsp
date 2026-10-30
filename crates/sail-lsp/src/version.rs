/// Shared by the CLI, initialize response and analyzer status.
pub(crate) fn version() -> String {
    format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("SAIL_LSP_BUILD_REVISION"))
}

#[allow(clippy::print_stdout)]
pub(crate) fn print_version() {
    println!("sail-lsp {}", version());
}
