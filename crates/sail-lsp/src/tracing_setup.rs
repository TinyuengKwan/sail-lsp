//! Tracing subscriber configuration for sail-lsp.
//!
//! Supports:
//! - `SAIL_LOG` env var for filter level (default: "warn")
//! - `SAIL_TRACE_TREE=1` for hierarchical span output (debug aid)
//!
//! # Examples
//!
//! ```bash
//! SAIL_LOG=debug cargo run --release              # verbose output
//! SAIL_LOG="hir_ty=trace" cargo run --release      # per-module filter
//! SAIL_TRACE_TREE=1 SAIL_LOG=info cargo run       # span tree
//! ```

use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

/// Initialize the global tracing subscriber.
///
/// Called once at startup in `main()`. After this, all `tracing::info!`,
/// `tracing::debug!`, `tracing::info_span!` etc. produce output.
pub fn setup() {
    let env_filter = EnvFilter::try_from_env("SAIL_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));

    let fmt_layer = tracing_subscriber::fmt::layer().with_target(true).with_writer(std::io::stderr);

    let tree_layer = if std::env::var("SAIL_TRACE_TREE").is_ok() {
        Some(
            tracing_tree::HierarchicalLayer::default()
                .with_indent_lines(true)
                .with_indent_amount(2)
                .with_writer(std::io::stderr),
        )
    } else {
        None
    };

    let subscriber = Registry::default().with(env_filter).with(fmt_layer).with(tree_layer);

    // Ignore error if a subscriber was already set (e.g., in tests).
    let _ = tracing::subscriber::set_global_default(subscriber);
}
