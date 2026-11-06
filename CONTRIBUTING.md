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

### Validation

Use the same source, compiler revision, corpus and build mode when comparing
results. Project loading, typecheck verdicts, diagnostic wording, expression-type
coverage and solver Unknown counts measure different things. A warning about an
undecided proof is not a refuted constraint; one zero-Unknown run is not a guarantee
that every proof fits the production budget.

| Purpose | Maintained tool | Inputs / limits |
| --- | --- | --- |
| Accepted/rejected programs | `conformance_check` | Upstream `test/typecheck/{pass,fail}`, `.expect` files and the extended-corpus oracle |
| Mutation detection | `mutation_check` | Upstream compiler verdicts for mutated, originally accepted programs |
| Active-project analysis and constraints | `analysis_stats` | A `.sail_project` manifest with the intended variables/modules |
| Per-file / flat-scope debugging | `corpus_check` | Directory of sources; does not establish project-module visibility parity |
| Completion recall | `completion_check` | Identifiers already written by the input corpus |
| Protocol positions | `protocol_check` | Cross-checks editor UTF-16 ranges against source tokens |
| Server-loop behavior | `cargo test -p sail-lsp server_check` | In-memory LSP client, versioned edits and requests |
| Incremental reuse | `incremental_stats`, `invalidation_matrix` | Real Salsa executions, changed diagnostics and retained results |
| Incremental parsing | `reparse_stats` | Seeded/full parses must have identical trees, tokens and errors |
| Query-cache policy | `query_cache_stats` | Weak ownership, eviction and recomputation; not per-query byte attribution |
| Interactive regression | `scripts/editing_replay.py`, `scripts/resource_replay.py` | Temporary projects, incomplete edits, reloads and finite resource-recovery workloads |
| Stdio latency and model sweeps | `scripts/editor_probe.py`, `scripts/latency_summary.py` | Real server processes; see [performance methods](docs/performance-gates.md) |
| Fixed performance gate | `scripts/performance_gate.py` | Same-host alternating binaries and the versioned policy in `benchmarks/` |
| Project semantics | `scripts/upstream_monitor.py` | Stock compiler and production evaluator; see [upstream monitoring](docs/upstream-monitoring.md) |

```sh
cargo run --release --example conformance_check -- --list /path/to/sail/test/typecheck
cargo run --release --example analysis_stats -- --diagnostics-only --constraint-report target/constraints.json /path/to/model.sail_project
cargo run --release --example completion_check -- /path/to/sail-riscv
cargo run --release --example incremental_stats -- 1600 32 signature
```

Examples are compiled by `cargo run`; rebuild before using a binary directly.
Use separate Cargo target directories for different checkouts, including baseline
and candidate builds. Source timestamps alone can leave another worktree's
artifacts appearing fresh.

Oracle inputs live in `crates/sail-lsp/test_data/`. The extended-corpus oracle uses
paths relative to the selected upstream `test/` directory, so moving a checkout
does not require rewriting its paths. Missing files and malformed verdicts fail
the check rather than reducing its scope. Regenerate verdicts when the upstream
compiler or bundled library changes:

```sh
export SAIL_DIR=/path/to/sail
export SAIL="$SAIL_DIR/_build/default/src/bin/sail.exe"
python3 scripts/regen_upstream_accepts.py "$SAIL_DIR/test" --sail "$SAIL"
cargo run --release --example mutation_check -- "$SAIL_DIR/test/typecheck/pass" --regen-oracle crates/sail-lsp/test_data/mutation-oracle.txt
cargo run --release --example mutation_check -- "$SAIL_DIR/test/typecheck/pass" --oracle crates/sail-lsp/test_data/mutation-oracle.txt
```

The generators preflight the compiler with an actual prelude-using program.
For an uninstalled compiler, `SAIL_DIR` is the source root containing `lib/`.
`mutation_check` excludes mutations upstream accepts and explicitly reports
unmutated inputs upstream already rejects.

For the opt-in manifest oracle, missing inputs are errors:

```sh
SAIL_ORACLE_BIN="$SAIL" SAIL_ORACLE_SRC="$SAIL_DIR" SAIL_ORACLE_RISCV=/path/to/sail-riscv \
  cargo test -p project-model upstream_oracle -- --ignored --nocapture
```

Synchronize the embedded standard library with `python3 scripts/sync_stdlib.py
/path/to/sail`, and verify it with `--check`. Its recorded upstream revision is
`crates/sail-lsp/data/sail-lib-revision`.

Keep raw measurements, audit reports and temporary probes under ignored
`target/` or in CI artifacts. Commit reusable tests and oracle inputs; document
current behavior and limits instead of historical completion counts.

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

## Checks before committing

```sh
cargo test --workspace --locked
cargo test --workspace --no-default-features --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
git diff --check
```

Run targeted behavioral checks for the changed feature. The
[main CI workflow](.github/workflows/ci.yaml) owns the Rust/editor platform matrices,
formatting, Clippy and spelling checks. Its conclusion also requires
[installation](.github/workflows/install.yaml),
[pinned corpus](.github/workflows/corpus.yml) and
[performance](.github/workflows/performance.yaml) gates. The separate
[latest-upstream monitor](.github/workflows/upstream.yml) is scheduled and does
not replace the pinned PR checks.

## VS Code extension development

The [extension guide](editors/code/README.md#development) owns build, test and
local packaging commands. For iteration, run `npm ci` and `npm run watch` in
`editors/code/`; `npm run format:check` checks TypeScript formatting.

## AI disclosure

This project uses AI assistants (Claude) for development. Commits authored or
co-authored with AI assistance include a `Co-Authored-By` trailer.

## License

By contributing, you agree that your contributions will be licensed under MIT
(see [LICENSE](LICENSE)).
