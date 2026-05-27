//! sail-lsp: Sail language server implementation.
//!
//! # Alignment with rust-analyzer
//!
//! ## ALIGN — patterns taken directly from RA
//!
//! - **Three-tier dispatch** (`on_sync_mut` / `on_sync` / `on`) in
//!   `handlers/dispatch.rs`.  `on::<ALLOW_RETRYING, R>` collapses RA's
//!   `on_latency_sensitive` into a const-generic flag.
//! - **GlobalState / GlobalStateSnapshot** split: mutable server state vs.
//!   cheap `Arc`-backed snapshot passed to handlers for cancellation isolation.
//! - **process_changes** — VFS changes buffered, applied in batch before
//!   dispatch (RA lazy pattern).
//! - **op_queue** — typed operation queue for deferred workspace reload and
//!   prime_caches.
//! - **TaskPool\<Task\>** — generic rayon-backed pool; `Task` enum in
//!   `main_loop.rs` (backed by rayon instead of RA's `stdx::thread::Pool`,
//!   no `ThreadIntent` priority).
//! - **DiagnosticCollection** — `native_syntax` + `native_semantic` slots with
//!   generation tracking (drops RA's `check`/flycheck slot).
//! - **prime_caches** — pre-warms salsa queries after workspace scan.
//! - **catch_cancelled** in `dispatch.rs` — `salsa::Cancelled` → `ContentModified`.
//!
//! ## CUSTOM — Sail-specific, no RA counterpart
//!
//! - **`progress.rs`** — `ProgressReporter` RAII guard for `$/progress`
//!   Begin/Report/End lifecycle.
//! - **`sail_stdlib.rs`** — embedded stdlib materialisation via `include_dir!`;
//!   resolves `$SAIL_DIR` or writes vendored files to a temp dir.
//! - **`code_action_helpers.rs`** — `code_action_kind_allowed` + lazy code
//!   action data helpers isolated from `lsp-types` in IDE crates.
//! - **`hover_ext.rs`** — `append_callable_context_to_hover` post-processing
//!   for Sail effect/callee footers.
//! - **`GlobalState::include_graph`** — `$include` dependency DAG used for
//!   cross-file diagnostic propagation and scope enforcement.
//! - **`GlobalState::workspace_index`** — `SymbolIndex` for O(1)
//!   name→location lookups without per-request file scans.
//! - **`send_request`** — fire-and-forget (RA uses a registered `ReqHandler`
//!   callback; sail-lsp drops server→client responses).
//!
//! See `DIFF_NOTES.md` (crate root) for the full field-by-field diff.

pub mod cli;
pub mod config;
pub mod lsp;

mod code_action_helpers;
mod diagnostics;
mod global_state;
mod handlers;
mod hover_ext;
mod main_loop;
mod mem_docs;
mod op_queue;
mod progress;
mod reload;
mod sail_stdlib;
mod task_pool;
pub mod tracing_setup;

// `mod handlers { pub(crate) mod dispatch; ... }`
// Re-export for crate-internal access:
pub(crate) use handlers::dispatch;

// `use self::lsp::ext as lsp_ext;`
pub(crate) use lsp::ext as lsp_ext;
pub(crate) use lsp::from_proto;
pub(crate) use lsp::to_proto;

#[cfg(test)]
mod integrated_benchmarks;
#[cfg(test)]
mod tests;

pub use main_loop::main;

/// Deserialize a value from JSON, returning an error on failure.
pub fn from_json<T: serde::de::DeserializeOwned>(
    what: &'static str,
    json: &serde_json::Value,
) -> anyhow::Result<T> {
    serde_json::from_value(json.clone())
        .map_err(|e| anyhow::anyhow!("Failed to deserialize {what}: {e}"))
}
