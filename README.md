# sail-lsp

A language server for [Sail](https://github.com/rems-project/sail), the ISA specification language. Works with any LSP-capable editor (VS Code, Neovim, Emacs, Helix, Zed, etc.).

Built in Rust, modeled after [rust-analyzer](https://github.com/rust-lang/rust-analyzer). Native analysis is self-contained. Optional Flycheck runs the upstream OCaml `sail` compiler for additional checks.

## Install

The repository includes user-scoped installers, standalone release packaging and
optional editor integration. **The first release with the new standalone assets
must be published before the prebuilt quick-install command can work.**

From a checkout today (Rust, CMake and a C++20 compiler with `std::format` support required):

```sh
git clone https://github.com/TinyuengKwan/sail-lsp.git
cd sail-lsp
sh install.sh --from-source --editor vscode
# Windows PowerShell: & .\install.ps1 --from-source --editor vscode
```

Once standalone assets are published, Linux/macOS users can run:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/TinyuengKwan/sail-lsp/main/install.sh | sh
```

Prebuilt installation embeds the Sail stdlib and Z3 and needs no Rust or Node.js.
Use `--editor vscode|zed|neovim|helix|emacs` for optional integrations, or keep your
existing LSP client configuration. See [installation](docs/installation.md) for
Windows, pinned/source/offline installs, platform requirements, remote editors,
upgrade/rollback/uninstall, and `sail-lsp doctor`.

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

Use `--editor vscode` to install a matching VSIX without Marketplace. See
[the extension documentation](editors/code/README.md) for settings and commands.

### Zed

Use `--editor zed` to prepare the pinned development extension, then select
**zed: install dev extension** in Zed. Details and remote setup are in
[installation](docs/installation.md#editors).

## Features

- **Navigation**: goto definition/declaration/implementation, find references, call hierarchy
- **Completion**: 11 providers (keywords, expressions, fields, patterns, pragmas, snippets, ...)
- **Hover**: type signatures, doc comments, effect annotations, bitfield layouts
- **Diagnostics**: 40 handlers (parse, semantic, type, effect) with quick fixes
- **Refactoring**: 51 code actions (extract, inline, generate, rename, organize imports, ...)
- **Formatting**: document + range + on-type + bitfield alignment
- **Highlights**: semantic tokens, document highlights, matching brace
- **Outline**: document symbols, folding ranges, selection range
- **Structural search/replace**: read-only expression matches via `sail-lsp/ssrSearch` and replacement previews via `experimental/ssr`; see [scope and limits](docs/architecture.md#ssr-engine-ide-ssr)

## Corpus quality

The [2026-11-02 unified baseline](docs/baselines/2026-11-02-unified.json)
records source/compiler provenance, commands, corpus scopes and limitations for
the earlier same-day worktree snapshot. Subsequent interaction changes and their
validation are tracked in the [interactive follow-up](docs/baselines/2026-11-02-interactive-followup.json).
In the unified snapshot, strict upstream typecheck is **276/276 accepted,
217/217 rejected**. The separate 372-file extended corpus has **0 false positives
and 0 false negatives**; 17 scope-only cases are counted separately. Fresh upstream
oracles retain the same verdicts. Ordered-project mutation checking detects
**274/274** upstream rejects.

The conformance tool fails if corpus directories or oracle sources are missing,
or the oracle is empty or malformed. Regenerate `docs/upstream-accepts.txt` when
moving the upstream checkout; missing inputs cannot silently reduce coverage.

The active RISC-V manifest contains 156 source files (29,968 lines), plus 40 active
included/library files. Its 81,543 expressions have no unknown or partially unknown
types, and diagnostics contain no errors (62 warnings, 6 hints). This run recorded
**2,914 proved, 0 refuted and 0 unknown** committed deferred numeric obligations.
This is a separate measure from expression types: finite solver budgets can change
both the deferred count and Unknown count. A zero-Unknown run is not a guarantee.
The [solver contention experiment](docs/baselines/2026-11-02-z3-contention.json)
reproduces budget exhaustion under parallel load; production budgets are unchanged.

Diagnostic starts match 205/216 upstream carets. The remaining 11 differences are
explained in the [diagnostic audit](docs/baselines/2026-10-30-diagnostic-audit.json);
acceptance/rejection agreement does not imply identical diagnostic wording.
That snapshot's protocol range probe covers all 158 physical model files and finds
0 mismatches in 470,394 ranges. This differs from the active manifest's 156-file
scope. Formatter checks cover 217 model/library files at two indentation widths:
434 checks, no skipped files, token changes, parse failures or idempotence failures.

The unified report also records stdio editing/reload measurements and synthetic
incremental checks. For 160 files and 32 edits, body changes reinfer one callable;
signature changes reinfer three expected callables and update dependent diagnostics.
These are observations on this host, not a speedup claim or real editor UI acceptance.
The [October 30 audit](docs/baselines/2026-10-30-ra-improvements.json) and earlier
dated reports remain historical snapshots, not current totals.

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
