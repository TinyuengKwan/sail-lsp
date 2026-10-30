# sail-lsp

A language server for [Sail](https://github.com/rems-project/sail), the ISA specification language. Works with any LSP-capable editor (VS Code, Neovim, Emacs, Helix, Zed, etc.).

Built in Rust, modeled after [rust-analyzer](https://github.com/rust-lang/rust-analyzer). Self-contained — does not call the upstream OCaml `sail` binary.

## Install

```bash
git clone https://github.com/TinyuengKwan/sail-lsp.git
cd sail-lsp
cargo build --release
# Binary: target/release/sail-lsp
```

## Setup

### Standard library

The Sail standard library is **embedded in the binary** — no extra setup needed.

The bundled sources track Sail `ac09f8870` (October 2026). To synchronize with
a local checkout and verify the bundle:

```bash
python3 scripts/sync_stdlib.py ../sail
python3 scripts/sync_stdlib.py --check ../sail
```

The upstream commit is recorded in `crates/sail-lsp/data/sail-lib-revision`;
extracted libraries use a directory specific to that revision. See
[CONTRIBUTING.md](CONTRIBUTING.md#validation) for how conformance against
upstream is measured.

To use a different version of the stdlib (e.g. a local checkout of the `sail` compiler), set `SAIL_DIR`:

```bash
export SAIL_DIR=/path/to/sail   # points to the sail compiler repo root
```

When set, `$SAIL_DIR/lib/` is used instead of the embedded copy.

### Neovim

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.sail_lsp then
  configs.sail_lsp = {
    default_config = {
      cmd = { "sail-lsp" },
      filetypes = { "sail" },
      root_dir = lspconfig.util.root_pattern("sail.proj", ".git"),
    },
  }
end

lspconfig.sail_lsp.setup({})
```

### Helix

```toml
# ~/.config/helix/languages.toml
[language-server.sail-lsp]
command = "sail-lsp"

[[language]]
name = "sail"
scope = "source.sail"
file-types = ["sail"]
roots = ["sail.proj"]
language-servers = ["sail-lsp"]
```

### VS Code

A dedicated extension is available at `editors/code/`. Install from a `.vsix`:

```bash
cd editors/code
npm install && npm run package
code --install-extension sail-lsp.vsix
```

This provides Sail syntax highlighting (TextMate grammar), language configuration,
and automatic `sail-lsp` server management. See `editors/code/README.md` for
configuration options.

### Zed

Extension: [TinyuengKwan/sail-zed](https://github.com/TinyuengKwan/sail-zed)

## Features

- **Navigation**: goto definition/declaration/implementation, find references, call hierarchy
- **Completion**: 11 providers (keywords, expressions, fields, patterns, pragmas, snippets, ...)
- **Hover**: type signatures, doc comments, effect annotations, bitfield layouts
- **Diagnostics**: 40 handlers (parse, semantic, type, effect) with quick fixes
- **Refactoring**: 51 code actions (extract, inline, generate, rename, organize imports, ...)
- **Formatting**: document + range + on-type + bitfield alignment
- **Highlights**: semantic tokens, document highlights, matching brace
- **Outline**: document symbols, folding ranges, selection range
- **Structural search/replace**: expression previews via `experimental/ssr`; see [scope and limits](docs/architecture.md#ssr-engine-ide-ssr)

## Corpus quality

The [2026-10-30 RA audit](docs/baselines/2026-10-30-ra-improvements.json)
records commands, source provenance, measurements and limitations. Strict upstream
typecheck is **276/276 accepted, 217/217 rejected**. The separate 372-file extended
corpus has **0 false positives and 0 false negatives**; 17 scope-only cases are
counted separately. Ordered-project mutation checking detects **274/274** upstream
rejects, including both existential constructor argument-order mutations.

The active RISC-V manifest contains 156 source files (29,968 lines), plus 40 active
included/library files. Its 81,543 expressions have no unknown or partially unknown
types, and diagnostics contain no errors (62 warnings, 6 hints). Deferred numeric
proof obligations are a separate measure: the final accepted run has **2,913 proved,
0 refuted and 4 budget-exhausted unknowns**. Earlier runs observed zero or one
unknown; a zero-unknown checkpoint is not a guarantee under finite solver budgets.
The three originally reported divisibility obligations are handled by an exact
non-SMT rule. The remaining budget-sensitive sites concern absolute-value bit widths
and loop-index bounds. Goals discharged earlier do not enter the deferred count.

Diagnostic starts match 205/216 upstream carets. The remaining 11 differences are
intentional, more precise ranges or related locations; the
[diagnostic audit](docs/baselines/2026-10-30-diagnostic-audit.json) records each case.
Acceptance/rejection agreement does not imply identical diagnostic wording.

The [2026-10-29 RA follow-up](docs/baselines/2026-10-29-ra-followup.json) and
[P2/P3 report](docs/baselines/2026-10-29-editor-p2-p3.json) preserve earlier checks.
For example, the historical protocol range probe covered 158 physical model files
and found 0 mismatches in 470,394 ranges; this differs from the active manifest's
156-file scope. Older dated reports remain snapshots, not current totals.

The server defaults to ordinary Sail mode. Set `workspace.strictBitvector` for
projects built with `--strict-bitvector`. Undefined-value and register-default
checks also apply in ordinary mode; an empty `list(bits(-1))` is permitted there,
as it is by upstream Sail. The [earlier baseline](docs/baselines/2026-10-24.json)
and [regression audit](docs/baselines/2026-10-24-regression-audit.json) retain the
before-fix measurements and compiler-mode investigation.

```bash
cargo run --release --example analysis_stats -- --diagnostics-only --constraint-report constraints.json /path/to/sail-riscv/model/riscv.sail_project
cargo run --release --example conformance_check -- --list /path/to/sail/test/typecheck
```

## Development

```bash
cargo test --workspace                  # run all tests
cargo test -p hir-ty                    # single crate
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks  # benchmarks
```

For incremental correctness and latency checks, run
`cargo run --release --example incremental_stats -- 1600 32 signature`.
The per-file contribution index preserves unrelated inference results after
signature edits; the probe verifies reuse and changes to consumer diagnostics.
Append `--profile` to inspect query timing and allocation counts.

For request latency, enable the dedicated tracing target and summarize its log:

```bash
SAIL_LOG=sail_lsp::latency=debug python3 scripts/editor_probe.py \
  --server target/release/sail-lsp --workspace ../sail-riscv \
  --cycles 20 --burst --output target/editor-burst.json
python3 scripts/latency_summary.py target/editor-burst.stderr.log
```

The trace separates snapshot creation, worker queueing, handler execution,
request completion, database cancellation/input/project updates, and background
result draining. Worker samples include retries. Server request totals start
when the main loop receives the request; the probe additionally measures
transport and unread connection messages. Compare repeated runs on the same
machine; a pipelined burst is a stress workload, not ordinary typing latency.

See [CONTRIBUTING.md](CONTRIBUTING.md) for code style and architecture details.

## License

MIT
