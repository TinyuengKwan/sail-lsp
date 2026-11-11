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

## `$[test]` functions and `main`

Upstream Sail has no test runner. `$[test]` marks a `unit -> unit` function
(Sail 0.20.2 rejects any other type), and the only consumer in the compiler is
the C backend, which emits `SAIL_TESTS`/`SAIL_TEST_NAMES` arrays for a host
harness to iterate. What the compiler itself can do with such a function is
evaluate it in its interpreter, and that is what the per-function entries run:

```sh
sail <project arguments> --is <script>     # script: test_name()\n:run\n:quit\n
```

Every `$[test]` function of a file in a loaded project gets a `Run test name`
entry and the `main` function a `Run main` entry, each placed at its
definition (`location`), with the project's own compiler arguments and the
interpreter script as `script`. VS Code shows a `▶` lens above each (also
behind `runnables.showCodeLens`) and offers them as tasks. The interpreter
exits 0 whatever happened: it prints `Result = ()` when the function evaluated
and `Error: <reason>` when an assertion failed or an exception escaped. A
function that does not type-check is reported as a type error on the `REPL`
pseudo-file.

With `sail-lsp.testExplorer` (off by default), the `$[test]` functions of open
files appear in VS Code's Test Explorer. Running them starts the interpreter
from the extension — the task terminal cannot be read back — and reads the
verdict from its output: passed on `Result = ()`, failed on `Error:`, a type
error or a non-zero exit. Output is attached to the test. There is no
`experimental/testing` protocol: nothing upstream exists for the server to
drive.

## Explicit test and export tasks

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

## VS Code tasks

Every process entry is also a task of `type: "sail"` (**Terminal: Run Task**,
`vscode.tasks.fetchTasks({ type: "sail" })`): the project tasks of every loaded
project and the `$[test]`/`main` entries of the open Sail files. Its definition
is what it runs — `command`, `args`, `cwd` and, for an interpreter entry,
`script` — so a `tasks.json` customization matches by those; the environment
is not part of the definition and stays in the private invocation file. Tests
are in the `test` group, exports in `build`.

`tasks.json` entries of the same type run as written:

```json
{
  "type": "sail",
  "label": "Export C",
  "command": "sail",
  "args": ["-c", "--project", "model.sail_project", "-o", "${workspaceFolder}/out/model"],
  "cwd": "${workspaceFolder}/model",
  "env": { "SAIL_DIR": "/opt/sail" }
}
```

`command`, `args`, `cwd` and `env` go through VS Code's variable substitution
and run as a process. An entry with `script` is an interpreter run: the script
is stored in a file whose path is appended to `args`, so end them with `--is`;
such an entry runs through the launcher and its values are not substituted.

Two problem matchers read the compiler's output into the Problems view and are
attached to every Sail task: `$sail` for the five-line `Type error:` /
`Syntax error:` / `Lexical error:` blocks (header, `file:line.col-col:`, the
source line, the caret line, the `| message` line) and `$sail-warning` for the
one-line `Warning: <message> at file:line.col-col:` header. Sail counts columns
from zero and VS Code from one, so a mark starts one character early. Colour
codes are tolerated; `--no-color` is not needed.

## Other clients

Other editors can request `experimental/runnables` with `textDocument` and an
optional `position`. Advertised kinds are `sail-check` and `shell`. The latter
has structured `program`/`args`/`cwd`/`environment`, an optional `script` to
store in a file whose path is appended to `args`, and an optional `location`
in the source; it should run as an argv process. The former supplies the scope
for `sail-lsp/runFlycheck`, cancelled with `sail-lsp/cancelFlycheck`. Entries
include stable IDs, fingerprints, related documents and optional disabled
reasons; clients should revalidate and check unsaved inputs before execution.
A fingerprint does not change when an entry merely moves in the file.


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
