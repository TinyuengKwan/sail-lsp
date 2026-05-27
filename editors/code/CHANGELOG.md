# Changelog

## 0.2.0

- Added Marketplace-ready extension metadata, support links, and release notes.
- Added platform-specific packaging support for bundling `sail-lsp` binaries into the VSIX.
- Added GitHub release automation for bundled multi-platform VSIX artifacts.
- Added optional automated VS Code Marketplace publishing from GitHub Actions using `VSCE_PAT`.
- Implemented the `Show Syntax Tree`, `View HIR`, and `View Item Tree` commands using the language server's custom requests.
- Added extension smoke tests and packaging documentation.

## 0.1.0

- Initial VS Code extension with Sail syntax highlighting, language configuration, and LSP client integration.
