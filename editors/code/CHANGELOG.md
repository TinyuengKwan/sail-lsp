# Changelog

## Unreleased

- Code actions apply the server's snippet edits, so an assist can leave the cursor where you type next; the client declares `snippetTextEdit` and `serverStatusNotification`.
- Added `Join Lines` (`Ctrl+Shift+J`), `Find Matching Brace`, `Move Item Up`/`Down` and an `Enter` key that continues comments and indents opened blocks through the server.
- Reference and implementation lenses run the extension's `sail-lsp.showReferences` command, which the client declares to the server; a lens arrives with protocol arguments the command converts.
- Added `View Include Graph`, the file-level counterpart of the module dependencies view.
- Typing `'` no longer inserts a closing quote (`forall 'n` is a type variable), and Enter inside a `/* */` comment continues it.

## 0.2.0

- Added Marketplace-ready extension metadata, support links, and release notes.
- Added platform-specific packaging support for bundling `sail-lsp` binaries into the VSIX.
- Added GitHub release automation for bundled multi-platform VSIX artifacts.
- Added optional automated VS Code Marketplace publishing from GitHub Actions using `VSCE_PAT`.
- Implemented the `Show Syntax Tree`, `View HIR`, and `View Item Tree` commands using the language server's custom requests.
- Added extension smoke tests and packaging documentation.

## 0.1.0

- Initial VS Code extension with Sail syntax highlighting, language configuration, and LSP client integration.
