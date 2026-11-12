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
//! - **TaskPool\<Task\>** — generic pool over RA's `stdx::thread::Pool`;
//!   `Task` enum in `main_loop.rs`. Every job says what it is for
//!   (`ThreadIntent`), and the pool counts what is extant.
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
//! - **`GlobalState::include_graph`** — `$include` dependency DAG used for
//!   cross-file diagnostic propagation and scope enforcement.
//! - **`GlobalState::symbol_index`** — `SymbolIndex` for O(1)
//!   name→location lookups without per-request file scans.
//! - **`send_request`** — registers a `ReqHandler` callback and routes each
//!   server→client response through the request queue.
//!
//! See `DIFF_NOTES.md` (crate root) for the full field-by-field diff.

pub mod cli;
pub mod config;
pub mod lsp;

mod analysis_health;
mod cache_control;
mod code_action_helpers;
mod diagnostics;
mod editor_commands;
mod flycheck;
mod formatting;
mod global_state;
mod handlers;
mod hover_actions;
mod main_loop;
mod manifest_editing;
mod mem_docs;
mod op_queue;
mod progress;
mod reload;
mod request_batch;
mod runnables;
mod sail_stdlib;
mod scoped_configuration;
mod shared_configuration;
mod task_pool;
pub mod tracing_setup;

// `mod handlers { pub(crate) mod dispatch; ... }`
// Re-export for crate-internal access:
pub(crate) use handlers::dispatch;

// `use self::lsp::ext as lsp_ext;`
pub(crate) use lsp::ext as lsp_ext;
pub(crate) use lsp::from_proto;
pub(crate) use lsp::to_proto;

/// What arrives on the wire — the fields of a diagnostic, a signature, a
/// refused rename — through the same server `server_check` drives.
#[cfg(test)]
mod diagnostics_check;
#[cfg(test)]
mod integrated_benchmarks;
/// The probe that enters where a client does — see its module docs for the
/// four defects that were invisible to the five that enter at `Analysis`.
#[cfg(test)]
mod runnables_check;
#[cfg(test)]
mod semantic_tokens_check;
#[cfg(test)]
mod server_check;
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

mod doctor;
mod install;
mod version;
