# Configuration

**Source:** [`crates/sail-lsp/src/config.rs`](../crates/sail-lsp/src/config.rs)

sail-lsp accepts LSP `initialize.initializationOptions` before project discovery
and `workspace/didChangeConfiguration` messages for subsequent updates.
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
  "sail-lsp.completion.limit": 100
}
```

## Neovim (lspconfig)

Pass settings via the `settings` table:

```lua
lspconfig.sail_lsp.setup({
  settings = {
    ["sail-lsp"] = {
      diagnostics = { enable = true },
      completion = { limit = 100 },
    },
  },
})
```

## Configuration reference

### `diagnostics`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Master enable/disable for diagnostics |
| `effectMismatch` | `bool` | `true` | Report effect diagnostics: `effect-violation` (an impure call in a pure function), `effect-mismatch`, `missing-effect-annotation` |
| `disabled` | `string[]` | `[]` | Diagnostic codes to suppress (e.g., `"unused-variable"`) |
| `disableExperimental` | `bool` | `false` | Suppress experimental diagnostics |
| `warningsAsHint` | `string[]` | `[]` | Diagnostic codes to show as hints |
| `warningsAsInfo` | `string[]` | `[]` | Diagnostic codes to show as info |
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

### `hover`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `docs` | `bool` | `true` | Show documentation in hover |
| `maxFieldsCount` | `integer or null` | `10` | Maximum struct/union members shown; `null` shows all |
| `maxEnumVariantsCount` | `integer or null` | `10` | Maximum enum variants shown; `null` shows all |
| `maxOverloadsCount` | `integer or null` | `10` | Maximum overload alternatives shown; `null` shows all |

### `semanticTokens`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `true` | Enable semantic token highlighting |

### `flycheck`

Runs the upstream Sail compiler on save, beside the server's own analysis.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `false` | Run `sail --just-check` on save |
| `command` | `string` | `"sail"` | The compiler binary |
| `extraArgs` | `string[]` | `[]` | Extra compiler arguments (`--config`, `--strict-bitvector`, ...) |

A save checks the project that lists the saved file (every loaded project,
for a file only reached through `$include`), one compiler run per project:
`sail --project <manifest> --just-check` with `workspace.projectVariables` as
`--variable` and `workspace.projectModules` as roots (`--all-modules` when
empty), run from the manifest's directory. A file no loaded project knows is
checked alone. A run's findings replace only that project's previous findings,
and a run overtaken by a later save of the same project is discarded.
Changing workspace or Flycheck configuration, disabling Flycheck, or reloading
projects clears old findings and invalidates pending runs. The next source save
starts a fresh check after loading completes. `workspace.strictBitvector` is
forwarded as `--strict-bitvector`.

Flycheck logs a warning and skips a run when it cannot preserve the IDE's
configuration: `workspace.target` (`--just-check` forces Sail's default target),
`workspace.includePaths` (no equivalent compiler search-path option), and, for
project runs, `generatedFiles` or CLI-inexpressible project variables. These
include string values `"true"`/`"false"`, leading/trailing whitespace, and variable
names containing `=`. Such a skipped run clears that target's previous findings;
it is not reported as a successful check.

A setting of the wrong type keeps its default and is named in a warning;
the other settings still apply.

### `workspace`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `strictBitvector` | `boolean` | `false` | Enable strict bitvector width well-formedness checks and the `STRICT_BITVECTOR` preprocessor symbol; use with projects compiled using `--strict-bitvector`. Changes invalidate analysis without editing source files. |
| `target` | `string?` | `null` | Compilation target for `$iftarget` directives |
| `primeCachesNumThreads` | `number` | `0` | Threads that pre-warm caches after loading; `0` uses half the machine's cores |
| `includePaths` | `string[]` | `[]` | Fallback include search paths, absolute or relative to workspace roots. |
| `projectFile` | `string?` | `null` | Manifest path relative to each workspace folder, or absolute. Auto-detection prefers root `sail.proj`, then a single root `*.sail_project`; otherwise checks immediate child directories for a unique project. |
| `projectVariables` | `object` | `{}` | Overrides for manifest variables. A JSON boolean is a boolean and a JSON string is always a string: `"true"` stays a string, unlike `sail --variable X=true`, which reads bare `true`/`false` as booleans. |
| `projectModules` | `string[]` | `[]` | Select root modules and what they transitively `requires` (a child module requires its parents; a parent does not bring its children), as `sail --project P.sail_project A B` does. Empty means all modules, as `--all-modules`. |
| `generatedFiles` | `object` | `{}` | Map manifest `__virtual` filenames to generated Sail source paths, absolute or relative to each workspace root. |

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
error and a diagnostic at the offending text of the manifest; they do not
silently index every file in the directory. In a workspace
with several folders, a folder whose manifest fails keeps the project it last
loaded (or none) while the other folders load normally. An explicitly empty
project stays empty.

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

The `unlinked-file` warning offers a quick fix that adds the file to a module's
`files`: after the module's last entry, outside any conditional, so it loads last
in that module. Modules whose files sit nearest the new file are offered; if
several are equally near, each is offered and none is preferred. The edit is
marked as needing confirmation, so clients that support it preview the manifest
change first.

Manifests themselves (`*.sail_project`, `sail.proj`) are never analysed as Sail.
In them the server answers go to definition (a module name to its header, a
`$variable` to its declaration, a path to the file) and completes module names
where a module is expected and variable names after `$`. These features read the
unsaved buffer; the loaded project follows the manifest on disk and reloads when
it is saved.

Source preprocessing follows the selected root order and active includes.
Only reached library files and the compiler's implicit `core/corelib.sail`
enter the semantic workspace. `$define` propagates through includes and later
files; `default Order` takes effect where declared. `target: null` takes the
else branch of `$iftarget`; named targets and `$target_set` aliases are honored.
Unresolved active includes are diagnosed, while missing inactive includes are
ignored. Unsaved include edits trigger discovery using editor contents.

Like the current Sail compiler, manifest `directory` expressions are evaluated
but do not prefix file paths; use explicit path expressions instead.

Manifest `__virtual` references use a registered source, just as the compiler
registers embedded contents. The standard `__virtual corelib.sail` resolves to
the effective Sail library's `core/corelib.sail`. For project-generated sources,
provide their producer's output explicitly:

```json
{
  "sail-lsp.workspace.generatedFiles": {
    "defs.sail": "build/generated_defs.sail"
  }
}
```

A manifest can then use `files __virtual defs.sail`. The LSP watches the mapped
file, accepts unsaved editor overlays, and uses its physical URI and source
ranges for diagnostics and navigation. Locations refer to the generated Sail
source, not the generator's templates. Mapping changes reload the project.
The producer must write the generated source; the LSP does not run generators.
An unregistered virtual name is a project error. A registered source missing on
disk follows the same partial-load and recovery behavior as other missing files.

The module graph controls selection, ordering and symbol visibility. A file
can use its own module and the public declarations of its direct `requires`,
plus the global scope. Requirements select dependencies transitively but do
not re-export their names. `before`/`after` only affect ordering; `private`
declarations are shared by files of the same module.
