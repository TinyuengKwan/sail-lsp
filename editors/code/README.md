# Sail Language Server for VS Code

Language support for the [Sail](https://github.com/rems-project/sail) ISA specification language in Visual Studio Code.

This extension provides syntax highlighting, diagnostics, goto definition, references, hover, inlay hints, code lenses, formatting, and other IDE features backed by the `sail-lsp` language server.

## Features

- Syntax highlighting and language configuration for `.sail`
- Diagnostics for parse, name-resolution, type, and effect errors
- Go to definition, declaration, implementation, references, and call hierarchy
- Hover with type information, docs, and Sail-specific semantic details
- Inlay hints, semantic tokens, document symbols, and folding ranges
- Commands for reloading the workspace, restarting the server, and viewing syntax/HIR/item-tree debug output
- Bundled `sail-lsp` binaries in platform-specific VSIX packages

## Installation

Install from the VS Code Marketplace when published, or install a platform-specific VSIX from a GitHub release.

If you build locally, use the VSIX that matches your platform so the extension can launch the bundled server binary.

## Bundled Server Binaries

The published extension is intended to ship as platform-specific packages, each containing the matching `sail-lsp` binary under `server/`.

If you prefer a custom server build, set `sail-lsp.server.path` to an explicit binary path. If that setting is unset, the extension tries:

1. `sail-lsp.server.path`
2. A bundled binary inside the extension
3. `sail-lsp` from `PATH`

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
| `sail-lsp.z3.timeoutMs` | `1000` | Constraint solver timeout |

Full configuration reference: [`docs/configuration.md`](../../docs/configuration.md)

## Commands

- `Sail: Reload Workspace`
- `Sail: Restart Server`
- `Sail: Show Syntax Tree`
- `Sail: View HIR`
- `Sail: View Item Tree`
- `Sail: Show Server Version`
- `Sail: Toggle Inlay Hints`

The syntax tree, HIR, and item tree commands are debugging aids intended for inspecting parser and semantic state for the active file.

## Development

```bash
cd editors/code
npm ci
npm run build
npm run test
```

To produce a platform-specific VSIX with a bundled server binary:

1. Build the Rust binary for that target.
2. Copy it into `editors/code/server/` with the packaging helper.
3. Run the matching `package:vsix:*` script.

Examples:

```bash
# Linux x64
cargo build --release -p sail-lsp --target x86_64-unknown-linux-gnu --target-dir target/vscode/linux-x64
cd editors/code
npm run package:server:linux-x64
npm run package:vsix:linux-x64

# macOS arm64
cargo build --release -p sail-lsp --target aarch64-apple-darwin --target-dir target/vscode/darwin-arm64
cd editors/code
npm run package:server:darwin-arm64
npm run package:vsix:darwin-arm64
```

## Publishing Checklist

- Create or verify the `sail-lsp` publisher in the VS Code Marketplace
- Authenticate with `npx @vscode/vsce login sail-lsp`
- Build per-platform bundled binaries
- Package platform-specific VSIX files with `--target`
- Publish with `npx @vscode/vsce publish --packagePath <vsix>` or upload through the Marketplace portal

GitHub Actions also provides `.github/workflows/release-vscode.yaml` to build bundled VSIX artifacts for the supported targets, attach them to a GitHub release, and optionally publish them to the VS Code Marketplace.

Required repository secret for Marketplace publishing:

- `VSCE_PAT`: Azure DevOps Marketplace personal access token with `Marketplace (Manage)` scope

For tag builds, the release workflow will publish to Marketplace automatically when `VSCE_PAT` is configured. For manual runs, enable the `publish_marketplace` input.

## Support

- Issues: <https://github.com/TinyuengKwan/sail-lsp/issues>
- Discussions: <https://github.com/TinyuengKwan/sail-lsp/discussions>

## License

MIT
