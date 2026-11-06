# Sail for Zed (repository development extension)

These sources are pinned from the upstream revision in [UPSTREAM](UPSTREAM) and
include local installation changes. The original MIT license is retained.

Run the repository installer with `--editor zed`, then use **zed: install dev
extension** and select the printed `<installation root>/editors/zed` directory.
See the [installation guide](https://github.com/TinyuengKwan/sail-lsp/blob/main/docs/installation.md)
for build requirements, remote use and uninstall.

The extension starts the server selected by `lsp.sail_lsp.binary.path` (the
installer writes the stable managed path), or `sail-lsp` on the worktree's PATH.
It also accepts the legacy `sail-lsp` settings key. It does not download a second
server or select `sail_server` implicitly.

```json
{
  "lsp": {
    "sail_lsp": {
      "binary": { "path": "/absolute/install/root/bin/sail-lsp", "arguments": [] },
      "settings": {}
    }
  }
}
```

Sail sources and Sail project manifests share the language server.
`initialization_options` and `settings` are forwarded to Sail LSP. For SSH projects,
install the executable on the remote host and configure a path available there.
Development installation uses Zed's official extension build workflow, with its
Rust/grammar build dependencies; it is not a fully offline, automatic extension
registration operation.
