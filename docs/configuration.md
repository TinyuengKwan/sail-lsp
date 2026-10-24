# Configuration

**Source:** [`crates/sail-lsp/src/config.rs`](../crates/sail-lsp/src/config.rs)

sail-lsp is configured via LSP `workspace/didChangeConfiguration` messages.
The exact format depends on your editor. Configuration is a JSON object under
the `sail-lsp` key; missing fields retain defaults.

## VS Code

Settings are exposed as `sail-lsp.*` in VS Code's settings UI. Example
`settings.json`:

```json
{
  "sail-lsp.diagnostics.enable": true,
  "sail-lsp.diagnostics.disabled": ["unused-variable"],
  "sail-lsp.inlayHints.typeHints": true,
  "sail-lsp.z3.timeoutMs": 2000
}
```

## Neovim (lspconfig)

Pass settings via the `settings` table:

```lua
lspconfig.sail_lsp.setup({
  settings = {
    ["sail-lsp"] = {
      diagnostics = { enable = true },
      z3 = { timeoutMs = 2000 },
    },
  },
})
```

## Configuration reference

### `diagnostics`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Master enable/disable for diagnostics |
| `effectMismatch` | `bool` | `true` | Enable effect mismatch warnings |
| `disabled` | `string[]` | `[]` | Diagnostic codes to suppress (e.g., `"unused-variable"`) |
| `disableExperimental` | `bool` | `false` | Suppress experimental diagnostics |
| `warningsAsHint` | `string[]` | `[]` | Diagnostic codes to show as hints |
| `warningsAsInfo` | `string[]` | `[]` | Diagnostic codes to show as info |
| `remapPrefix` | `object` | `{}` | Path prefix remapping for diagnostic locations |
| `maxDiagnosticsPerFile` | `number` | `128` | Max diagnostics per file (0 = unlimited) |

### `inlayHints`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Master enable for inlay hints |
| `typeHints` | `bool` | `true` | Show type hints for let bindings |
| `parameterHints` | `bool` | `true` | Show parameter name hints at call sites |
| `effectHints` | `bool` | `true` | Show effect annotation hints |
| `maxLength` | `number?` | `25` | Max hint text length before truncation |
| `closingBraceHintsMinLines` | `number?` | `6` | Min lines for closing brace hints |

### `completion`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Master enable for completions |
| `addCallParenthesis` | `bool` | `true` | Add `()` after function completion |
| `postfix` | `bool` | `true` | Enable postfix completion templates |
| `snippets` | `bool` | `true` | Enable snippet completions |
| `autoimport` | `bool` | `false` | Enable auto-import (`$include`) on the fly |
| `limit` | `number` | `200` | Maximum number of completion items |
| `fullFunctionSignatures` | `bool` | `false` | Show full signatures in completions |

### `codeLens`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Master enable for code lenses |
| `references` | `bool` | `true` | Show reference counts |
| `implementations` | `bool` | `true` | Show implementation counts |
| `runnables` | `bool` | `true` | Show run lenses for `$[test]` functions |

### `hover`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `docs` | `bool` | `true` | Show documentation in hover |
| `keywords` | `bool` | `true` | Show keyword documentation |
| `actions` | `bool` | `true` | Hover actions (go-to-definition, etc.) |

### `semanticTokens`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Enable semantic token highlighting |

### `z3`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `timeoutMs` | `number` | `1000` | Z3 solver timeout in milliseconds (0 = no timeout) |

### `workspace`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `maxFiles` | `number` | `10000` | Maximum files to scan in workspace |
| `target` | `string?` | `null` | Compilation target for `$iftarget` directives |
| `includePaths` | `string[]` | `[]` | Fallback include search paths, absolute or relative to workspace roots. |
| `projectFile` | `string?` | `null` | Manifest path relative to each workspace folder, or absolute. Auto-detection prefers root `sail.proj`, then a single root `*.sail_project`; otherwise checks immediate child directories for a unique project. |
| `projectVariables` | `object` | `{}` | Boolean/string overrides for manifest variables. |
| `projectModules` | `string[]` | `[]` | Select root modules, their descendants and transitive requirements. Empty means all modules. |

For example, VS Code can select the RMEM configuration with:

```json
{
  "sail-lsp.workspace.projectFile": "model/riscv.sail_project",
  "sail-lsp.workspace.projectVariables": { "RMEM": true },
  "sail-lsp.workspace.projectModules": []
}
```

Changes reload the active file set. Conditional branches are evaluated, and
`requires`, `before`, and `after` determine file order. Only `requires` adds
modules to the selected dependency closure. Invalid manifests report a server
error and retain the last successfully loaded workspace; they do not silently
index every file in the directory. An explicitly empty project stays empty.

Opening a repository root also discovers a nested manifest such as
`model/riscv.sail_project`. Only one level of child directories is searched;
hidden directories are excluded. Deeper projects require `workspace.projectFile`.
Within a nested directory, `sail.proj` takes precedence over `*.sail_project`.
If several projects remain, set `workspace.projectFile` explicitly; their
sources are not merged into a single implicit project.

The server logs the paths of successfully loaded manifests. If no project is
found, it reports a warning instead of treating directory/file-name order as
Sail declaration order. Only manifest roots and their reached includes belong
to the semantic project; opening a file does not add it to that project.
Unlinked files retain syntax diagnostics and local editing features, and receive
an `unlinked-file` warning explaining how to configure the project. Type checking
and other project-dependent diagnostics resume once the file joins a loaded
project. Set `diagnostics.disabled` to include `unlinked-file` to hide that warning.
This also applies when opening a single file without a workspace folder.

Source preprocessing follows the selected root order and active includes.
Only reached library files and the compiler's implicit `core/corelib.sail`
enter the semantic workspace. `$define` propagates through includes and later
files; `default Order` takes effect where declared. `target: null` takes the
else branch of `$iftarget`; named targets and `$target_set` aliases are honored.
Unresolved active includes are diagnosed, while missing inactive includes are
ignored. Unsaved include edits trigger discovery using editor contents.

Like the current Sail compiler, manifest `directory` expressions are evaluated
but do not prefix file paths; use explicit path expressions instead. Generated
`virtual` files are not supported and produce an explicit project error.
The module graph controls selection, ordering and symbol visibility. A file
can use its own module and the public declarations of its direct `requires`,
plus the global scope. Requirements select dependencies transitively but do
not re-export their names. `before`/`after` only affect ordering; `private`
declarations are shared by files of the same module.
