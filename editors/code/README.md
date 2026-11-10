# Sail Language Server for VS Code

Language support for the [Sail](https://github.com/rems-project/sail) ISA specification language in Visual Studio Code.

This extension provides syntax highlighting, diagnostics, goto definition, references, hover, inlay hints, code lenses, formatting, and other IDE features backed by the `sail-lsp` language server.

## Features

- Syntax highlighting and language configuration for `.sail`, and for project manifests (`*.sail_project`, `sail.proj`)
- In manifests: go to a module's declaration, a `$variable`'s, or a listed file; module and variable completion; load errors underlined where they are
- A quick fix for a file no project loads: add it to a module's `files`, choosing the module when several fit, with a preview before the manifest changes
- Diagnostics for parse, name-resolution, type, and effect errors
- Go to definition, declaration, implementation, references, and call hierarchy
- Hover with type information, docs, and Sail-specific semantic details
- Inlay hints, semantic tokens, document symbols, and folding ranges
- A status bar item showing whether the server is loading, healthy, or failed to load the project; click it for the full status
- Commands for project navigation, reloading the workspace, restarting the server, and viewing syntax/HIR/item-tree debug output
- Bundled `sail-lsp` binaries in platform-specific VSIX packages

## Installation

Use the repository installer with `--editor vscode` to install the matching VSIX
and register the managed server, without a Marketplace dependency. See the
[installation guide](../../docs/installation.md) for source builds and the first
standalone release requirement. A platform-specific release VSIX can also be
installed directly with `code --install-extension FILE`.

If you build locally, use the VSIX that matches your platform so the extension can launch the bundled server binary.

## Bundled Server Binaries

The published extension is intended to ship as platform-specific packages, each containing the matching `sail-lsp` binary under `server/`.

If you prefer a custom server build, set `sail-lsp.server.path` to an explicit binary path. Server selection follows this order:

1. `sail-lsp.server.path`
2. The installer's registration (`SAIL_LSP_CONFIG_HOME/vscode.json`, otherwise
   `~/.config/sail-lsp/vscode.json` following XDG, or `%APPDATA%/sail-lsp/vscode.json`)
3. A bundled binary inside the extension
4. `sail-lsp` from `PATH`

An invalid managed registration produces a visible error instead of silently
launching another binary. Restart the server after installation or rollback.

## Configuration

All settings live under the `sail-lsp.*` namespace.

Common settings:

| Setting | Default | Description |
| --- | --- | --- |
| `sail-lsp.server.path` | `null` | Override the server binary path |
| `sail-lsp.server.extraEnv` | `null` | Extra environment variables for the server |
| `sail-lsp.trace.server` | `off` | LSP protocol tracing level |
| `sail-lsp.diagnostics.enable` | `true` | Enable diagnostics |
| `sail-lsp.inlayHints.enable` | `true` | Enable inlay hints |
| `sail-lsp.completion.enable` | `true` | Enable completions |
| `sail-lsp.codeLens.enable` | `true` | Enable code lenses |
| `sail-lsp.semanticTokens.enable` | `true` | Enable semantic highlighting |
| `sail-lsp.flycheck.enable` | `false` | Run the upstream compiler on save |

Full configuration reference: [`docs/configuration.md`](../../docs/configuration.md)

## Commands

- `Sail: Reload Workspace`
- `Sail: Run Project Task` — check a loaded project, run configured test scripts or export the model
- `Sail: Cancel All Sail Tasks` — terminate process tasks and compiler checks
- `Sail: Restart Server`
- `Sail: Show Server Status` — the server's status report (also on the status bar item)
- `Sail: Open Project Manifest` — the manifest entry that lists the active file, or the entry of the file that `$include`s it
- `Sail: Go to Parent Module` — the manifest module that lists the active file, or the `$include` that names it
- `Sail: Show Syntax Tree`
- `Sail: View HIR`
- `Sail: View Item Tree`
- `Sail: Show Server Version`
- `Sail: Toggle Inlay Hints`
- `Sail: Explain Formatting` — show effective options, project style and no-op reasons in a read-only view

Formatting uses local text edits and respects a project's `.sailfmt.toml`.
Use `sail-lsp fmt --check <file-or-directory>` in CI to check the same style;
see [formatting configuration](../../docs/configuration.md#formatting).
Enable `editor.formatOnType` for input-triggered formatting and
`editor.formatOnSave` for formatting on save. If several Sail formatters are
installed, choose `Lunarwwww.sail-lsp` as `editor.defaultFormatter` for `[sail]`.

The syntax tree, HIR, and item tree commands are debugging aids intended for inspecting parser and semantic state for the active file.

Project tasks also appear as CodeLens links. Configure `sail-lsp.runnables.tasks`
for tests/exports and `sail-lsp.runnables.showCodeLens` to control the links.
Commands use the loaded project's variables/modules and the selected compiler.
See [project task configuration](../../docs/runnables.md) for argv/context
placeholders, script integration and stale/unsaved-input behavior.

## Development

```bash
cd editors/code
npm ci
npm run build
npm run test
```

To produce and install a local VSIX on the current platform:

```sh
# At repository root; add --no-modify-path to manage PATH yourself.
sh install.sh --path "$PWD" --editor vscode
# Windows: & .\install.ps1 --path (Get-Location).Path --editor vscode
```

This requires Node.js 22+/npm in addition to the Rust/CMake/C++ source build tools.
For packaging without installation, see the verified standalone-payload example
in [installation](../../docs/installation.md#release-and-ci-maintenance).
`npm run package:local` builds a host-specific vendored binary and VSIX.

Release builds use `.github/workflows/release-server.yaml`. The VSIX reuses the
verified standalone binary, including Windows `.exe` and static Z3. GitHub assets
can be published independently of Marketplace. Marketplace publishing is an
explicit opt-in and requires `VSCE_PAT`; tag builds do not publish there by default.
The manual `release-vscode.yaml` entry point delegates to the same workflow.

## Support

- Issues: <https://github.com/TinyuengKwan/sail-lsp/issues>
- Discussions: <https://github.com/TinyuengKwan/sail-lsp/discussions>

## License

MIT
