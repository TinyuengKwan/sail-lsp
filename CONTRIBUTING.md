# Contributing to sail-lsp

sail-lsp follows rust-analyzer's crate layout and coding conventions. This
document covers how to build, test, and contribute.

## Building

```bash
# Debug build (all crates)
cargo build --workspace

# Release build (optimized LSP binary)
cargo build --release
# Binary at: target/release/sail-lsp
```

The Z3 SMT solver feature is enabled by default. To build without it:

```bash
cargo build --workspace --no-default-features
```

## Testing

```bash
# Run all tests
cargo test --workspace

# Single-crate iteration (faster feedback loop)
cargo test -p parser
cargo test -p syntax
cargo test -p hir-def
cargo test -p hir-ty
cargo test -p ide
cargo test -p ide-assists
cargo test -p ide-diagnostics
cargo test -p ide-completion
cargo test -p sail-lsp

# Run a specific test
cargo test -p hir-ty -- test_name
```

### Integrated benchmarks

Benchmarks live in `crates/sail-lsp/src/integrated_benchmarks.rs` and require
a sail-riscv checkout at `../sail-riscv` (sibling directory).

```bash
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks --nocapture
```

### Corpus validation

The full sail-riscv corpus check validates against 158 files. Run explicitly:

```bash
cargo run --example corpus_check -- /path/to/sail-riscv
cargo run --example corpus_check -- --workspace /path/to/sail-riscv
```

Expected: **0 errors, 0 false positives**. All remaining warnings should be
true positives (genuinely unused variables, code style suggestions).

See [docs/corpus-validation.md](docs/corpus-validation.md) for the full
fix history and methodology.

### Analysis stats

Performance benchmarking tool (mirrors RA's analysis-stats):

```bash
cargo run --release --example analysis_stats -- /path/to/sail-riscv
```

## Code style conventions

sail-lsp is architecturally aligned with rust-analyzer. Follow these conventions:

### Crate dependencies

- **hir** does NOT depend on **ide-db** (RA architecture invariant).
- **ide-diagnostics** and **hir-ty** are LSP-free (no lsp-types dependency).
- Only **sail-lsp** depends on lsp-types; all IDE crates use internal types
  from `ide-db::ide_types`.

### Module organization

- Public API types are re-exported from `lib.rs` via `pub use`.
- Internal modules use `pub(crate)`.
- Handler registries use static arrays (e.g., `handlers::all()`).

### Naming

- Follow RA naming: `CompletionContext`, `AssistContext`, `DiagnosticsContext`.
- Salsa queries end in `_query` (e.g., `item_tree_query`, `infer_callable_query`).
- Database traits: `DefDatabase`, `HirDatabase`.

### Error handling

- Diagnostic handlers use `err_recover` pattern (never panic on bad input).
- Request handlers wrapped in `catch_unwind` for crash isolation.
- `salsa::Cancelled` is a panic caught at dispatch level.

### Testing patterns

- Unit tests live in the same file as the code they test (`#[cfg(test)]` mod).
- Integration tests and benchmarks use `#[cfg(test)]` modules.
- Corpus tests use `#[ignore]` for slow tests.

### Assist / diagnostic / completion handlers

- **Assists**: one file per handler in `ide-assists/src/handlers/`, registered
  in `handlers/mod.rs` via `all()`.
- **Diagnostics**: one file per handler in `ide-diagnostics/src/handlers/`,
  with `AnyDiagnostic` variants in `hir/src/diagnostics.rs`.
- **Completions**: one file per provider in `ide-completion/src/completions/`,
  dispatched from `completions.rs`.

### Formatting

- `cargo fmt` before committing. The workspace `rustfmt.toml` sets
  `use_small_heuristics = "Max"`.
- No compiler warnings allowed (`cargo build --workspace` must be warning-free).

### Clippy rules

CI denies the following clippy lints:

- `clippy::dbg_macro`
- `clippy::todo`
- `clippy::print_stdout`
- `clippy::print_stderr`

Use `eprintln!` only in CLI entry points. For debug logging, use `tracing`.

## CI

CI runs on every PR and push to `main` (`.github/workflows/ci.yaml`). Jobs:

- **Rust** (ubuntu, windows, macos): `cargo nextest run --workspace` with default
  features (z3-solver enabled). Ubuntu also tests with `--no-default-features`.
- **Formatting**: `cargo fmt -- --check`.
- **Clippy**: workspace-wide with denied lints (see Code style above).
- **VS Code extension**: `npm ci`, typecheck, format check, production build, `.vsix` package.
- **Typos**: spell check via `crate-ci/typos`.
- **Conclusion**: gate job that fails if any dependency failed.

## VS Code extension development

The extension source is at `editors/code/`.

```bash
cd editors/code
npm install           # install dependencies
npm run watch         # dev build with file watcher
npm run build         # one-shot build with sourcemaps
npm run package       # produce sail-lsp.vsix
npm run typecheck     # TypeScript type checking
npm run format:check  # Prettier format check
```

To test in VS Code, open `editors/code/` as a workspace and press F5 to launch
an Extension Development Host.

## AI disclosure

This project uses AI assistants (Claude) for development. Commits authored or
co-authored with AI assistance include a `Co-Authored-By` trailer.

## License

By contributing, you agree that your contributions will be licensed under MIT
(see `LICENSE.md`).
