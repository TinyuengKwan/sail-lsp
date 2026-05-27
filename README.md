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

Validated against [sail-riscv](https://github.com/riscv/sail-riscv) (158 files, 30,018 lines): **0 errors, 0 false positives, 0 crashes**.

```bash
cargo run --example corpus_check -- /path/to/sail-riscv
```

## Development

```bash
cargo test --workspace                  # run all tests
cargo test -p hir-ty                    # single crate
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks  # benchmarks
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for code style and architecture details.

## License

MIT
