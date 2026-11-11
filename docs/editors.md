# Other editors

sail-lsp speaks the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
over stdio: start the `sail-lsp` binary with no arguments for `.sail` sources
(language id `sail`) and for project manifests (`*.sail_project`, `sail.proj`;
language id `sail-project`). Settings travel as the `sail-lsp` section of
`initialize.initializationOptions` and `workspace/configuration`; the keys are
the VS Code settings without the `sail-lsp.` prefix (see
[configuration](configuration.md)). A `.sail-lsp.toml` in the workspace folder
is read by the server itself and works in every editor.

`sail-lsp install --editor neovim,helix,emacs,zed` writes the integrations below
that are marked *managed*; see [installation](installation.md#editors). The
others are written by hand from what the server needs: a command, two
language ids, and an optional settings object.

## Emacs

### Eglot (managed)

Emacs 29 includes Eglot. `editors/templates/emacs.el` is what the installer
writes: derived `sail-mode`/`sail-project-mode` unless you already have them,
the server in `eglot-server-programs`, `eglot-ensure` on both modes, and
commands for the server's own requests — `sail-lsp-analyzer-status`,
`sail-lsp-open-project-manifest`, `sail-lsp-parent-module`,
`sail-lsp-join-lines`. Settings go through `eglot-workspace-configuration`,
best set per project in `.dir-locals.el`:

```elisp
((nil . ((eglot-workspace-configuration
          . (:sail-lsp (:diagnostics (:disabled ["unused-variable"])
                        :completion (:limit 100)))))))
```

### lsp-mode

`editors/templates/emacs-lsp-mode.el` registers the client with
`lsp-register-client` (`:server-id 'sail-lsp`, activated on the `sail` and
`sail-project` language ids), declares two settings with
`lsp-register-custom-settings` as a pattern for the rest, sends the `sail-lsp`
section as initialization options, and defines the same four commands on
`lsp-request`. Load it from your init file; it expects `sail-lsp` on `PATH`.

## Vim and Neovim

### Neovim 0.11+ (managed)

`editors/templates/neovim.lua` uses the built-in `vim.lsp.config` /
`vim.lsp.enable` with no plugin, registers both filetypes, finds the workspace
root from `sail.proj`, `*.sail_project` or `.git`, and defines `:Sail*` commands
over the server's source actions. Settings are the table under
`settings["sail-lsp"]`; [configuration](configuration.md#neovim) shows one.

### nvim-lspconfig

Without the managed file, register the server yourself; nvim-lspconfig has no
entry for it. The same `vim.lsp.config` call works, with the name of your choice:

```lua
vim.lsp.config("sail_lsp", {
  cmd = { "sail-lsp" },
  filetypes = { "sail", "sail_project" },
  root_markers = { "sail.proj", ".git" },
  settings = { ["sail-lsp"] = { completion = { limit = 100 } } },
})
vim.lsp.enable("sail_lsp")
vim.filetype.add({
  extension = { sail = "sail", sail_project = "sail_project" },
  filename = { ["sail.proj"] = "sail_project" },
})
```

### vim-lsp

```vim
if executable('sail-lsp')
  au User lsp_setup call lsp#register_server({
        \   'name': 'sail-lsp',
        \   'cmd': {server_info->['sail-lsp']},
        \   'allowlist': ['sail', 'sail_project'],
        \   'workspace_config': {'sail-lsp': {'completion': {'limit': 100}}},
        \ })
endif
au BufRead,BufNewFile *.sail setfiletype sail
au BufRead,BufNewFile *.sail_project,sail.proj setfiletype sail_project
```

### coc.nvim

In `coc-settings.json`:

```json
{
  "languageserver": {
    "sail": {
      "command": "sail-lsp",
      "filetypes": ["sail", "sail_project"],
      "rootPatterns": ["sail.proj", "*.sail_project", ".git"],
      "settings": { "sail-lsp": { "completion": { "limit": 100 } } }
    }
  }
}
```

coc sends `settings` as the configuration the server pulls; the filetype
lines above apply here too.

## Helix (managed)

The installer adds a `sail-lsp-managed` language server to `languages.toml` and
two languages that use it: `sail` (file type `sail`) and `sail-project` (globs
`*.sail_project` and `sail.proj`), both with `sail.proj` and `.git` as roots.
By hand, the same two blocks are:

```toml
[language-server.sail-lsp]
command = "sail-lsp"

[[language]]
name = "sail"
scope = "source.sail"
file-types = ["sail"]
roots = ["sail.proj", ".git"]
language-servers = ["sail-lsp"]

[[language]]
name = "sail-project"
scope = "source.sail-project"
file-types = [{ glob = "*.sail_project" }, { glob = "sail.proj" }]
roots = ["sail.proj", ".git"]
language-servers = ["sail-lsp"]
```

Settings go in `[language-server.sail-lsp.config]` as the `sail-lsp` section
(`config = { sail-lsp = { completion = { limit = 100 } } }`). Helix has no Sail
grammar of its own; highlighting is independent of the server.

## Sublime Text

Install the [LSP package](https://packagecontrol.io/packages/LSP) and add a
client in its settings (**Preferences: LSP Settings**). Sublime has no Sail
syntax definition, so the selector below applies to plain text files with
these extensions:

```json
{
  "clients": {
    "sail-lsp": {
      "enabled": true,
      "command": ["sail-lsp"],
      "selector": "text.plain",
      "settings": { "sail-lsp": { "completion": { "limit": 100 } } }
    }
  }
}
```

Restrict `selector` to a Sail syntax's scope once you have one installed.

## Kate

Kate's built-in LSP Client plugin takes servers in its **User Server Settings**:

```json
{
  "servers": {
    "sail": {
      "command": ["sail-lsp"],
      "rootIndicationFileNames": ["sail.proj"],
      "highlightingModeRegex": "^Sail$",
      "initializationOptions": { "sail-lsp": { "completion": { "limit": 100 } } }
    }
  }
}
```

`highlightingModeRegex` names the highlighting mode the server attaches to;
Kate ships none for Sail, so define one or match the mode you use for `.sail`
files.

## Zed (managed)

`editors/zed` is a development extension the installer prepares; see
[installation](installation.md#editors). Settings go under `lsp.sail_lsp.settings`
in Zed's `settings.json`, as the `sail-lsp` section.

## VS Code

The extension in `editors/code` is the reference client, including the
server's protocol extensions, which other editors reach only through the
source actions. Its settings are documented in
[configuration](configuration.md#vs-code).
