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
- **SSR**: structural search-replace with `$name` placeholders

## Corpus quality

The [2026-10-24 P1 baseline](docs/baselines/2026-10-24-p1.json) records
276/276 accepted and 217/217 rejected upstream typecheck cases, using the suite's
`--strict-bitvector` mode. The active RISC-V manifest contains 156 source files
(29,968 lines): 81,543 expressions have no unknown or partially unknown types,
and project diagnostics contain no errors. Seven false positives remain in the
separate extended corpora; acceptance/rejection agreement does not imply identical
diagnostic wording or locations.

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

See [CONTRIBUTING.md](CONTRIBUTING.md) for code style and architecture details.

## License

MIT
