# Project tasks

VS Code provides **Sail: Run Project Task**, project task CodeLens links, and
**Sail: Cancel All Sail Tasks**. `sail-lsp.runnables.showCodeLens` controls the
links. Projects are selected from the loaded model that owns the active file,
including its `$include` files. Shared files offer their projects separately.
Labels show the manifest path and selected root modules.

Every loaded project has a compiler check entry. It uses the existing Flycheck
queue with that project's effective variables/modules, the configured compiler,
extra check arguments, protocol negotiation, timeout, cancellation and diagnostic
isolation. Explicit checks work with `flycheck.enable=false`; that setting
controls automatic checks. Pending reloads and unsaved inputs still prevent a
check from presenting previous disk contents as current.

Sail has no universal test discovery/build convention. Declare the existing
test scripts and exports you want under `sail-lsp.runnables.tasks`, for example:

```json
{
  "sail-lsp.runnables.tasks": [
    {
      "label": "Export C",
      "kind": "export",
      "command": "${sail}",
      "args": ["-c", "${projectArgs}", "-o", "${projectDir}/model"]
    },
    {
      "label": "Model tests",
      "kind": "test",
      "project": "model/riscv.sail_project",
      "command": "python3",
      "args": ["${workspaceRoot}/scripts/test_model.py"],
      "environment": {
        "SAIL_LSP_PROJECT_CONTEXT": "${projectContext}",
        "SAIL_COMPILER": "${sail}"
      }
    }
  ]
}
```

The test script must consume that context to configure its build/test invocation;
the LSP does not infer arbitrary scripts' argument conventions. Every task must
explicitly receive `${projectArgs}` or `${projectContext}`, through an argument
or environment value. `project` optionally restricts a task to a manifest path
relative to its workspace root (or an absolute path).

| Placeholder | Meaning |
| --- | --- |
| `${sail}` | Configured `flycheck.command` executable |
| `${project}` | Absolute loaded manifest path |
| `${projectDir}` | Manifest directory, also the process working directory |
| `${workspaceRoot}` | This project's workspace root |
| `${projectArgs}` | Argument-array expansion preserving manifest, CLI variables, selected roots and strict-bitvector mode; no `--just-check` |
| `${projectContext}` | One JSON value containing the full typed context |

The bundled launcher starts the configured executable with an argument array.
Spaces remain within individual arguments. It reads a private invocation
file after VS Code resolves its own task variables; model values such as the
literal `${workspaceFolder}` therefore stay unchanged. Payload files are private
to the user, retained in a bounded cache for recent tasks and removed when the
extension deactivates. Placeholders expand once and shell syntax is not
interpreted. The environment inherits the editor's environment, then
`server.extraEnv`, then the task's `environment`. Declared task environment
values are redacted in analyzer status and diagnostic reports.

On Windows a fixed `cmd.exe` bootstrap wraps VS Code's bundled Electron runtime
so the task terminal reports process start, exit and cancellation correctly.
Only fixed bootstrap code and private path variables enter that command; task
arguments and environment values are read from JSON after shell expansion.
Delayed expansion and shell autorun are disabled. No separate Node installation
is required, and launcher-only variables are removed from the child's environment.

`${projectArgs}` must occupy an entire argument. It excludes
`flycheck.extraArgs`: supply the desired test/export options explicitly. If a
setting cannot round-trip through stock Sail's CLI (a string `"true"`, generated
files, include search paths or target), the task is disabled with a reason. A
task adapter can instead consume `${projectContext}`. Check-context v1 is
check-only and is not reused as an export protocol.

Context version 1 contains `manifest`, `workspaceRoot`, `compiler`, `modules`,
typed `variables`, `strictBitvector`, `target`, absolute `includePaths` and
absolute `generatedFiles` mappings. It contains paths rather than generated
source contents; the producer continues to own those files.

Before execution, the command re-discovers and compares the selected task.
Changed task definitions or loaded configuration invalidate old selections.
Unsaved manifests and pending reloads disable discovery entries. VS Code checks
workspace trust and unsaved related documents before launch. Output, failures
and termination use its normal task terminal. Discovery and edits never execute
tasks automatically.

Other editors can request `experimental/runnables` with `textDocument` and an
optional `position`. Advertised kinds are `sail-check` and `shell`. The latter
has structured `program`/`args`/`cwd`/`environment` and should run as an argv
process. The former supplies the scope for `sail-lsp/runFlycheck`, cancelled
with `sail-lsp/cancelFlycheck`. Entries include stable IDs, fingerprints,
related documents and optional disabled reasons; clients should revalidate
and check unsaved inputs before execution.


## Standard LSP clients

Sail source actions expose each available task through
`workspace/executeCommand`. Discovery, project arguments, typed variables and
stale/dirty-input checks are shared with VS Code. Compiler checks use Flycheck.
For clients without a task terminal API, test/export tasks run in a dedicated
server thread, with one fallback process task at a time. Commands and arguments
are passed directly to the operating system; shell syntax is not interpreted.
The environment inherits the server process and then the task's overrides.

Results open in a read-only temporary preview, with exit status and at most
1 MiB from each output stream. `runnables.timeoutMs` (default `300000`) limits
these fallback tasks. The **Sail: Cancel project task** source action cancels
and reaps the process group and cancels scoped compiler checks. Server shutdown
also cancels and joins the fallback worker. Interactive input and a native
terminal are provided by VS Code's task integration; the standard-client
fallback captures output and supplies no stdin.
