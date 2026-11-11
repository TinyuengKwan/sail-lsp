## Where the server comes from

1. `sail-lsp.server.path`, if set (`${workspaceFolder}` and `${env:NAME}` are substituted).
2. The managed installation registered by `sail-lsp install --editor vscode`.
3. The server bundled with a platform VSIX.
4. `sail-lsp` on `PATH`.

Changing `sail-lsp.server.path` or `sail-lsp.server.extraEnv` prompts for a restart; **Sail: Show Server Version** tells which binary runs.
