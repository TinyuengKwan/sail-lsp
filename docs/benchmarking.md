# Benchmarking

sail-lsp includes integrated benchmarks and standalone analysis tools modeled
after rust-analyzer's `analysis-stats` CLI and corpus validation.

## Integrated benchmarks

The primary benchmarking mechanism lives in
`crates/sail-lsp/src/integrated_benchmarks.rs` as a `#[cfg(test)]` module.
This follows RA's pattern of in-crate benchmarks with full access to server
internals.

### Usage

```bash
# Run benchmarks (requires sail-riscv checkout at ../sail-riscv)
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks

# With output visible
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks --nocapture
```

The `RUN_SLOW_BENCHES` environment variable gates execution so normal
`cargo test` is fast. Without it, the benchmark tests are skipped.

### What's measured

The integrated benchmarks exercise the full analysis pipeline on a real corpus
(sail-riscv), measuring:

- Workspace load + salsa input registration
- Parse all files
- ItemTree construction
- Type inference (sequential and parallel)
- Diagnostic computation
- Symbol index build
- prime_caches timing (parallel speedup)

## analysis_stats (example binary)

Full analysis pipeline benchmark with per-phase timing and metric output.
Mirrors RA's `cargo xtask metrics` / `analysis-stats` CLI tool.

### Usage

```bash
# Basic run (release build recommended)
cargo run --release --example analysis_stats -- /path/to/sail-project

# Verbose: per-file detail
cargo run --release --example analysis_stats -- -v /path/to/sail-project
```

### Output format

Results are printed in RA's `METRIC:name:value:unit` format for CI integration:

```
=======================================================
sail-lsp analysis-stats
=======================================================

Workspace: 158 files
Load + salsa inputs:      13 ms
METRIC:workspace_load:13:ms
Total lines:           30018
Parse all:               137 ms (0 errors)
METRIC:parse_all:137:ms
ItemTree all:             91 ms (5472 items)
METRIC:item_tree_all:91:ms
Infer all:             13586 ms (3680 callables, 460 type errors)
METRIC:infer_all:13586:ms
...
```

### Measured phases

| Phase | What it measures | Typical time (sail-riscv) |
|-------|-----------------|---------------------------|
| Workspace load | VFS registration + salsa inputs | ~13 ms |
| Parse all | `parse_file()` for all files | ~137 ms |
| ItemTree all | `item_tree_query()` for all files | ~91 ms |
| Infer all | `infer_callable_query()` for all callables | ~13,500 ms (seq) |
| Diagnostics all | `file_diagnostics()` for all files | ~348 ms |
| Symbol index | `WorkspaceSymbolIndex::add_file()` | ~101 ms |
| Hover avg | `analysis.hover()` sample (10 files) | ~15 us |
| prime_caches parallel | Full prime with rayon (`N` threads) | ~1,600 ms (24 threads) |
| prime_caches sequential | Full prime (1 thread) | ~14,200 ms |

### Parallel speedup

The benchmark automatically measures parallel vs sequential `prime_caches`:

```
prime_caches (24 threads):  1620 ms
prime_caches (1 thread):  14244 ms
Speedup:                  8.8x
```

### Profiling slow callables

Callables taking >50ms are reported:

```
  Slowest callables (>50ms):
        236 ms  vext_fp_utils_insts.sail#2412
        228 ms  vext_fp_utils_insts.sail#2413
         65 ms  vext_fp_utils_insts.sail#2424
```

### Latency distribution

Second-pass timing (salsa cache warm) shows p50/p90/p99:

```
  Infer latency distribution:
    p50:       0 us
    p90:       0 us
    p99:       0 us
    max:      12 us
```

## corpus_check (example binary)

Validates sail-lsp against a Sail project (typically sail-riscv, 158 files).
See [corpus-validation.md](corpus-validation.md) for methodology.

```bash
# Single-file mode (crash-isolated subprocesses)
cargo run --example corpus_check -- /path/to/sail-riscv

# Workspace mode (cross-file type resolution)
cargo run --example corpus_check -- --workspace /path/to/sail-riscv
```

Expected output: `Parse errors: 0 / Type errors: 0 / Crashes: 0`.

## LSP integration test

Manual LSP protocol test via Python (requires `python3`):

```bash
# Build release binary
cargo build --release

# Test push diagnostics on large workspace
python3 -c "
import json, subprocess, time, select
# ... (see examples in agent/ plans)
"
```

Key latencies to verify:

| Operation | Target |
|-----------|--------|
| Init handshake | < 10 ms |
| Workspace scan + index | < 500 ms |
| Hover response | < 5 ms |
| Completion response | < 10 ms |
| didChange -> push diagnostics | < 100 ms |
| Pull diagnostics | < 10 ms |

## Reference: sail-riscv baseline (2026-05-27)

158 files, 30,018 lines, 3,680 callables, 5,472 symbols.

| Metric | Value |
|--------|-------|
| Workspace load | 13 ms |
| Parse all | 137 ms |
| ItemTree all | 91 ms |
| Infer all (sequential) | 13,571 ms |
| Infer all (24 threads) | 1,620 ms |
| Diagnostics all | 348 ms |
| Symbol index | 101 ms |
| Hover avg | 15 us |
| Total analysis | 14,279 ms |
| Corpus errors | 0 |
| Corpus warnings (TP) | 72 |
| Corpus false positives | 0 |
