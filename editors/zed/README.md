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


Use **editor: toggle code actions** on a Sail source to find **Sail:** project
and analysis actions: choose a project/module/include context, open its manifest,
explain a type or unresolved name, preview actual preprocessing, and run/cancel
project tasks. Explanations and preprocessing open read-only temporary snapshots
with standard source links. The **Sail Analysis Preview** language keeps those
links on the language-server integration. See [project and analysis commands](../../docs/installation.md#project-and-analysis-commands)
and [task limits](../../docs/runnables.md#standard-lsp-clients).

**Sail: Explain formatting** shows effective style, configuration path and why
formatting would change the file or return no edits. The editor and
`sail-lsp fmt --check` share [.sailfmt.toml configuration](../../docs/configuration.md#formatting).
