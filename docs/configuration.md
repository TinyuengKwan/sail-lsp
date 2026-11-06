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

## Neovim

For the managed integration (Neovim 0.11+), configure its server name:

```lua
vim.lsp.config("sail_lsp_managed", {
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

With `autoimport` enabled, background cache priming also prepares the import
catalog, including inactive candidate files. Enabling the setting schedules
priming without reloading the project. Candidate changes invalidate the same
per-file queries used by completion; importing still requires resolving and
validating the selected edit. Requests arriving before priming finishes can
still pay the initial catalog cost.

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

Runs the upstream Sail compiler on save and after a successful project reload,
beside the server's own analysis. `Sail: Recheck Projects` requests another run without a save.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | `bool` | `false` | Run `sail --just-check` on save |
| `command` | `string` | `"sail"` | The compiler binary |
| `extraArgs` | `string[]` | `[]` | Extra compiler arguments (`--config`, `--strict-bitvector`, ...) |
| `timeoutMs` | `integer` | `30000` | Maximum elapsed time per compiler invocation |
| `debounceMs` | `integer` | `200` | Coalesce repeated saves before starting the compiler |

A save checks the project that lists the saved file (every loaded project,
for a file only reached through `$include`), one compiler run per project:
`sail --project <manifest> --just-check` with `workspace.projectVariables` as
`--variable` and `workspace.projectModules` as roots (`--all-modules` when
empty), run from the manifest's directory. A file no loaded project knows is
checked alone. A run's findings replace only that project's previous findings,
and a run overtaken by a later save of the same project is discarded.
Changing workspace or Flycheck configuration, disabling Flycheck, or reloading
projects clears old findings and cancels pending runs. Once loading completes,
enabled checks restart automatically. Changing Flycheck settings also starts a
fresh check. A project has at most one running compiler; a newer save replaces
its pending check and terminates an obsolete run. Disabling Flycheck or shutting
down the server cancels its workers and reaps running compiler processes.
Editing a source invalidates that source's affected project checks immediately,
including results still in flight. Compiler checks use saved files: while an
affected source has unsaved changes, checking that project is deferred and its
status says to save the affected files. Other projects retain their results.
Push and pull diagnostic reports both include compiler findings. Exact duplicate
findings from shared projects are merged; different diagnostic sources and fix
metadata are preserved. An undecided native constraint remains a warning and
reports the recorded blocker when available; it is not treated as a proof of a
type error.
`workspace.strictBitvector` is
forwarded as `--strict-bitvector`.

Stock Sail 0.20.2/0.20.3 cannot express `workspace.target`, `includePaths`, project
`generatedFiles`, or some typed project variables through its check CLI. For these
configurations Flycheck probes `--check-context-version`. A compiler reporting
protocol `1` receives `--check-context <json>`; older compilers report a visible
skipped check and clear that target's previous findings. They never silently
check a different configuration. Ordinary configurations continue to use the
stock CLI and do not require this extension.

The optional [Sail companion patch](../patches/sail-check-context-v1.patch)
implements protocol v1 against Sail commit
`ac09f8870b9d85e528e2a3436150b03b9d805d40` (0.20.3). **This is a local compiler
extension, not an interface supplied by released upstream Sail.** Apply it in a
separate Sail checkout, build `dune build --release src/bin/sail.exe`, and point
`flycheck.command` at that executable. For example, from the sail-lsp checkout:

```sh
git -C /path/to/separate/sail apply --check "$PWD/patches/sail-check-context-v1.patch"
git -C /path/to/separate/sail apply "$PWD/patches/sail-check-context-v1.patch"
```

After applying the patch to the chosen checkout, the resulting compiler must
print `1` for `--check-context-version`. The server does not patch or install a
compiler automatically.

The JSON object contains `version: 1`, `target` (string or null), `includePaths`
(absolute paths, in search order), `variables` (JSON booleans or strings), and
`generatedFiles` (virtual names mapped to absolute physical source paths).
Strings such as `"true"`, `"false"`, and values with surrounding spaces retain
their exact types and contents. File-relative includes still win over fallback
paths. Generated files retain their physical diagnostic locations and resolve
relative includes from their own directory. The target selects preprocessing
branches while retaining the check-only backend; no code generator is run.
The capability probe and check share the configured timeout and cancellation.

Root selection retains upstream semantics: in Sail 0.20.3, selected roots affect
`--list-files` and the final AST, but `Frontend.load_modules` typechecks other
declared modules as well. An error in such a module can therefore legitimately
appear in Flycheck even when it is outside the selected output closure.

The server status reports queued/running, passed, diagnostics, failed, skipped,
cancelled, and timed-out checks separately. Spawn errors and nonzero exits without
parseable diagnostics remain visible as failures. The VS Code status tooltip
includes the reason and a retry action; other clients can inspect
`sail-lsp/analyzerStatus`. `sail-lsp/recheck` reruns every loaded project and
open standalone file. The scoped `sail-lsp/runFlycheck`,
`sail-lsp/cancelFlycheck`, and `sail-lsp/clearFlycheck` notifications accept an
optional `textDocument`; with one they affect only the containing project, and
without one they affect all targets. Cancel preserves the last published
compiler diagnostics, while clear removes them. The VS Code extension exposes
these operations from the command palette.

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
| `linkedProjects` | `array` | `[]` | Explicit manifests, including several in one folder. Entries are paths or objects with `projectFile`, `projectVariables`, and optional `projectModules`. Variables override the common settings; an explicit modules array replaces the common roots. Empty uses normal discovery. |
| `activeProject` | `string?` | `null` | Preferred manifest for semantic queries on shared files. Absolute or workspace-relative. Defaults to the first loaded manifest in path order; an invalid selection reports the fallback. |
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

An absolute `linkedProjects` entry belongs to its deepest containing workspace
folder (or the first folder in path order if it is outside every folder).
Relative generated-source paths use that folder; duplicate absolute entries
load once.

In VS Code, **Sail: Select Analysis Project** chooses the shared-file context.
**Sail: Show Module Dependencies** opens the **Sail Module Dependencies** tree in the
Explorer. Expand a project to see active and inactive modules, and a module to
follow its `requires` edges and files. Shared dependencies appear under every
requiring module; cycles are marked and stop expanding. Project and file entries
open the manifest or source. Run the command again (or use the view's refresh
action) to fetch the current graph. Other clients can request
`sail-lsp/projectGraph` for the same project/module data.

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

Open manifests with unsaved changes produce a `serverStatus` warning naming the
manifest and explaining that analysis still uses the last loaded project. VS Code
shows this in the Sail status bar tooltip. Saving, closing or reverting the buffer clears
the reminder; project-loading errors remain errors. Buffers restored with text
different from disk are also reported, even before the first edit notification.

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

**Sail: Explain Analysis at Cursor** shows the saved project configuration that
produced the current model, the owning module and its direct requirements,
whether the cursor is in an active preprocessing region, and the current name
resolution. For an unresolved name it lists matching loaded declarations and
known visibility blockers, such as a private declaration or a missing direct
requirement. It does not guess which combination of edits would make a name
resolve. Local bindings, multiple include contexts, pending reloads and project
load failures are shown separately. Other clients can request
`sail-lsp/explainAnalysis` with `TextDocumentPositionParams`.

**Sail: Show Server Status** includes passive cache accounting: physical and
semantic-view inputs, unique retained source/parse-seed text bytes, parse seeds,
live weak tree references, and live analysis snapshots (including the status
request). Reading status does not build the symbol index or include graph.
With SMT enabled it also reports the bounded solver-result cache and the number
of live/previous threads that have created solvers. Each such worker can retain
a native thread-local solver context; the count is not an estimate of its size.
The automatic interactive worker pool uses at least two and at most eight
threads. Unlike rust-analyzer's ordinary query workers, Sail workers can own a
native Z3 context; bounding the pool prevents high core counts from multiplying
that retained cost without limit. `workspace.primeCachesNumThreads` continues
to control the separate, temporary cache-priming pool.
These counters do not measure the sizes of inference results, trees, indexes or
allocator overhead. The allocator heap estimate and operating-system RSS are
different measurements; neither alone proves a leak or successful reclamation.

### `formatting`

`lineWidth` (default `100`) is a soft line width. A long single-line function
body moves after the definition `=` before short parameters are split. Existing
multiline parameter and expression layouts, aligned mapping tables, and
instruction mapping clauses are preserved. Long single-line `@` and `^` chains
wrap before an operator with one additional indentation level, keeping complete
operands together. Parentheses, comments, and string contents remain intact;
indivisible operands may exceed the width. Whole-document formatting removes leading/repeated blank lines and
inserts one final newline unless the client explicitly overrides the newline
options; comment and string contents are preserved.

Indentation follows the client's `tabSize` and `insertSpaces`. The VS Code
extension defaults Sail documents to two spaces; other editors should configure
these options explicitly when editing sail-riscv.

## Include contexts, symbols and refactoring

When a physical file is included more than once, **Sail: Select Include
Context** chooses the semantic view used by position-based reads such as hover,
completion and navigation. The status bar shows the chosen module, or `Auto`,
when several instances exist. The choice belongs to the open document and is
cleared on close. An instance that disappears or is inactive at the cursor
falls back to an active view. This choice does not authorize edits to shared
source. Rename, SSR and assists compare all relevant active include instances.
A shared edit is available only when the affected names resolve to the same
physical declarations and each context produces the same physical changes.
Equivalent declarations have their references collected across all semantic
views; edits to a shared source token are emitted once. The check also covers
shared references reached from an unshared parent file. Field and constructor
identities retain the exact selected member declaration, including shadowed
same-name records.

Differences in bindings, replacement text, generated files or available quick
fixes make the operation unavailable. Inactive branches are checked only in the
contexts where they are active; a replacement spanning incompatible conditional
regions is rejected. Missing resolution, including ownerless type variables,
is not treated as proof of identity. Auto-include keeps its separate preview
of the post-insertion bindings and diagnostics across every include visit, and
repeats that validation when resolving the action. Selecting a read context does
not bypass these conditions. Rename returns document versions when the client supports
them; SSR previews are versioned, and lazy code actions revalidate source
fingerprints before resolving. Clients must honor those version preconditions
when applying retained edits.

Workspace symbol queries match ordered abbreviations as well as exact names,
prefixes and substrings. Exact and prefix matches rank first. The default limit
is 128, with an explicit maximum of 1024. Optional whitespace-separated filters:

- `#mr`: only type declarations matching `mr`.
- `mr kind:function`: filter by `function`, `val`, `struct`, `enum`, `union`,
  `type`, `mapping` or `register`.
- `mr scope:workspace` / `scope:library` / `scope:all`: select the source scope.
- `mr project:riscv`: restrict to projects whose manifest path contains `riscv`.
- `mr limit:20`: limit results after scope filtering and physical deduplication.

Results point at the declared identifier and show the owning module/manifest
and source path. The project graph refreshes after an installed model changes;
module nodes open declarations in the current manifest buffer, including
unsaved edits. Saving still controls when the project model changes.

**Sail: Structural Replace** validates `pattern ==>> replacement`, prepares a
review panel with per-file diffs, and offers explicit Apply/Cancel actions.
Sources remain unchanged until Apply. The server supplies every source text
used to compute the preview; the client checks those texts and keeps the files
open with version guards. Editing or closing a source invalidates the preview.
Applying edits uses the normal document-change and diagnostic pipeline.

Expression rules remain the default. Other fragment categories use a prefix:

```text
type: bitvector($n) ==>> vector($n, bool)
item: val $f : $a -> $b ==>> val $f : $a -> option($b)
pattern: ($a, $b) ==>> ($b, $a)
let $x = $value in $body ==>> let $x = $value in $body + 1
```

Fixed type and value references must keep their semantic identity. Fixed item
names identify the selected declaration; placeholders allow matching different
names. Item rules replace one declaration and preserve its declared name;
use Rename to change a symbol's name. Binding-pattern rules preserve the set of
bound names. Introduced local/type binders are freshened, and captured references
must retain their owners; rules that would capture a free variable or let a
binding escape are not applied. Use binder placeholders to match different
spellings. A repeated fixed binder name whose correspondence is ambiguous can
prevent an unsafe match. Library sources, shared include tokens and replacements
that discard comments or leave invalid syntax remain protected.

File/folder moves update resolved `$include` paths and loaded manifests using
versioned edits. Relative paths are rebased when the referencing file moves;
ordinary strings/comments and unrelated same-basename files are untouched.
Literal lists, concatenations and both conditional branches are rewritten.
Computed scalar paths use a guard for the currently resolved spelling so other
variable values retain their meaning. A computed list, a dynamically rooted
manifest move or an unrepresentable include path is refused atomically when it
cannot be rewritten safely. Virtual-file aliases remain configuration entries;
update `workspace.generatedFiles` when moving their physical source files.

The optional [compiler patch](../patches/sail-check-context-v1.patch) defines the
`Check_context` parser, schema and tests. Its `doc/check-context.md` describes the
proposal's contract and build commands. Stock 0.20.2/0.20.3 use the legacy CLI for
representable contexts; extended contexts require a capability reply of exactly
`1`. A different protocol version is skipped with a visible reason.

## Quick fixes

Availability depends on the current source, visible declarations and client edit
capabilities. Actions are checked again when resolved; stale actions must be
requested again after editing.

| Diagnostic | Offered action / limitation |
| --- | --- |
| Unused local binding | Remove or rename the unused binding |
| Incomplete match | Add missing cases; generated bodies may contain `undefined` |
| Missing record fields | Fill expressions or patterns; generated values may need implementation |
| Unresolved name / missing include | Add an include for a visible declaration when a safe edit is available |
| Missing closing delimiter | Insert the closing delimiter where supported |
| Bitvector used as an integer | Offer `unsigned` / `signed` when a compatible callable is in scope; the user chooses signedness |
| Unreachable code | Remove the selected unreachable statement |

There is no general conversion for arbitrary type mismatches or automatic repair
of argument counts and effect violations. Duplicate/shadowed declarations do not
get a single-token rename fix; use semantic Rename to update their references.
An include can affect preprocessing, order and scope, so unused-include removal
is not offered through a text-only heuristic.

## Analysis cache policy

Query results can be limited independently through `sail-lsp.cache.parseCapacity`,
`environmentCapacity`, `inferenceCapacity`, and `bodyCapacity`. These count results
per query, not bytes. Defaults retain all parse, type environment and inference
results, and 512 callable-body/source-map results. Zero disables eviction for a
family. Usage tracking remains enabled so changing a limit can reclaim existing
results as well as subsequently queried results.

`sail-lsp.cache.memoryLimitMb` defaults to zero (disabled). A positive value is a
soft process RSS high-water mark in MiB. At an idle boundary, with no worker or
retained analysis snapshot, the server samples RSS at most once every five seconds.
Above the target, unbounded families use 32 results and bounded families use one
quarter of their configured capacity (at least one). It restores configured limits
when RSS falls below 80% of the target. Repeated pressure does not repeatedly divide
capacities. The server collects the LRU values and asks the allocator to release
unused pages where supported. Configuration changes take effect at the next safe
idle boundary; they do not cancel requests to force reclamation.

This is not a hard process limit: file inputs, indexes, shared trees, retained
snapshots, native solver contexts and allocator overhead can prevent reaching the
target. RSS includes native allocations. Analyzer status and diagnostic reports
separately show Z3's process-wide allocation estimate, which is a subset of process
memory and must not be added to RSS. If RSS is unavailable, automatic pressure is
not inferred from incomplete text-byte counts.

Use the `query_cache_stats` tool listed in
[contributor validation](../CONTRIBUTING.md#validation) to compare retention,
recomputation and revisit latency. Weak ownership counts track query results;
heap estimates are not per-query byte attribution.

## Exporting an analysis diagnostic report

VS Code provides **Sail: Export Diagnostic Report**, also linked from the Sail
status bar tooltip. It saves a local JSON file containing the client/server build,
project configuration and load status, active file version/context, latest native
analysis failures, published solver Unknown reason counts, bounded slow/error
request history, and cache/memory observations. The command remains available when
the server executable is missing or cannot start; that report records the client
startup failure instead.

The server endpoint is `sail-lsp/diagnosticReport` with an optional `textDocument`.
Reports do not collect source buffers, request parameters or the process environment.
Declared project-task environment values are redacted.
They do contain local paths, effective settings and error messages, so inspect the
saved file before sharing it. Export does not run inference or invoke the compiler.

Normal edit/client cancellation is recorded separately from a worker panic or a
Salsa propagated panic. A current-generation native diagnostic failure produces a
warning status and clears the failed phase's obsolete findings; a later successful
wave restores its status. Stale waves cannot overwrite newer findings or health.
Solver Unknown is incomplete proof, not a refuted constraint or an internal crash;
its reason is attached to the diagnostic data and summarized for the latest
published semantic wave. Counts describe observed current-generation waves, not
all attempted work across the lifetime of the process.

## Project checks, tests and exports

`runnables.tasks` defines explicit test/export commands and
`runnables.showCodeLens` controls their VS Code links. Loaded projects also offer
an explicit compiler check, which works independently of automatic Flycheck.
See [project tasks](runnables.md) for configuration, typed context, execution and
cancellation behavior.
