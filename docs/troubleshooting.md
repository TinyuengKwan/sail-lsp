# Troubleshooting

## Check the version

```bash
sail-lsp --version
```

If you built from source, rebuild with `cargo build --release` to pick up
recent fixes.

## Check the logs

sail-lsp logs to stderr. In VS Code, open `Output > Sail Language Server`
to see log messages. For more detail:

```bash
# Set log level via environment variable
SAIL_LSP_LOG=info sail-lsp

# Or via VS Code settings
"sail-lsp.server.extraEnv": { "SAIL_LSP_LOG": "info" }
```

Use `SAIL_LSP_LOG=hir_ty=debug` to see type inference details, or
`SAIL_LSP_LOG=sail_lsp=debug` for LSP message tracing.

## Standard library not found

sail-lsp embeds the Sail standard library. If you see missing-prelude errors,
the embedded copy may be incompatible with your Sail project. Override with:

```bash
export SAIL_DIR=/path/to/sail   # points to the sail compiler repo root
```

When set, `$SAIL_DIR/lib/` is used instead of the embedded copy.

## Nothing works / no diagnostics

Check that the workspace root is correct. sail-lsp looks for `.sail` files
starting from the workspace folder. If your project uses a `sail.proj` file,
ensure it's in the workspace root.

To verify the server sees your files, check the initial log output for the
file count:

```
Loading workspace from /path/to/project...
Loaded 158 project files in 45 ms
```

If the count is 0, the workspace root is wrong.

## Batch analysis

To check whether a problem is in the LSP transport or in the analysis engine,
bypass LSP and run batch analysis:

```bash
# Full corpus check (parse + type check all files)
cargo run --example corpus_check -- /path/to/project

# Performance metrics
cargo run --release --example analysis_stats -- /path/to/project
```

If `corpus_check` reports errors but the upstream Sail compiler accepts the
code, please file an issue with a minimal reproduction.

## Crash isolation

If sail-lsp crashes on a specific file, the crash is caught and reported as
a diagnostic. Other files continue working. To investigate:

```bash
# Run with backtrace
RUST_BACKTRACE=1 sail-lsp
```

## Filing issues

When filing an issue, include:

1. `sail-lsp --version` output
2. The `.sail` file (or minimal reproduction) that triggers the problem
3. The error/warning message from the editor
4. Log output with `SAIL_LSP_LOG=info`

An ideal reproduction:

```bash
git clone https://github.com/user/repo.git && cd repo && git checkout <hash>
sail-lsp --version
cargo run --example corpus_check -- .
```
