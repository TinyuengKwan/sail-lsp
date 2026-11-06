# sail-lsp

A language server for [Sail](https://github.com/rems-project/sail), the ISA
specification language. Built in Rust with a rust-analyzer-style architecture.
Native analysis is self-contained; optional Flycheck runs the upstream Sail
compiler for additional checks.

## Install

From a checkout (requires Rust, CMake and a C++20 compiler with `std::format`):

```sh
git clone https://github.com/TinyuengKwan/sail-lsp.git
cd sail-lsp
sh install.sh --from-source
# Windows PowerShell: & .\install.ps1 --from-source
```

Add `--editor vscode|zed|neovim|helix|emacs` for managed editor integration.
Building the VS Code extension from source also requires Node.js/npm; Zed uses
its development-extension installation step.

**Prebuilt quick installation requires a release with the standalone assets.**
Once those assets are published, Linux/macOS users can run:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/TinyuengKwan/sail-lsp/main/install.sh | sh
```

The prebuilt server embeds the Sail standard library and Z3. See
[installation](docs/installation.md) for Windows, source/offline installs,
[editor setup](docs/installation.md#editors), remote sessions, upgrade, rollback,
uninstall and `sail-lsp doctor`. Existing LSP client configurations can continue
to launch `sail-lsp` from PATH.

## Configuration

The embedded library needs no additional setup. Its upstream revision is recorded
in [sail-lib-revision](crates/sail-lsp/data/sail-lib-revision). To use another
checkout, set `SAIL_DIR` to its root; the server reads `$SAIL_DIR/lib/`.

Settings are documented in [configuration](docs/configuration.md), including
project selection, solver limits, completion and diagnostics. The server defaults
to ordinary Sail mode; use `workspace.strictBitvector` for projects built with
`--strict-bitvector`.

## Features

- Navigation: definitions, declarations, implementations, references and call hierarchy
- Completion: expressions, fields, patterns, pragmas, snippets and auto-import
- Hover: signatures, documentation, effects and bitfield layouts
- Diagnostics and [quick fixes](docs/configuration.md#quick-fixes)
- Refactoring: extraction, inlining, generation, rename and import organization
- Formatting: document, range and on-type formatting, with bitfield alignment
- Semantic highlighting, document symbols, folding and selection ranges
- [Project tasks](docs/runnables.md), module dependencies and include-context selection
- Structural search and replacement previews; see [scope and limits](docs/architecture.md#ssr-engine-ide-ssr)

VS Code settings and commands are listed in the
[extension guide](editors/code/README.md). For loading errors, logs and runtime
identification, see [troubleshooting](docs/troubleshooting.md).

## Development

```sh
cargo build --workspace
cargo test --workspace --locked
```

[CONTRIBUTING.md](CONTRIBUTING.md) covers checks, corpus validation and performance
tools. [Architecture](docs/architecture.md) describes the query and semantic model.
[Upstream monitoring](docs/upstream-monitoring.md) and
[performance gates](docs/performance-gates.md) define their separate test scopes.
Historical pass counts do not establish current coverage or rust-analyzer parity.

## License

[MIT](LICENSE)
