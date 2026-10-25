# Troubleshooting

## Check the server binary

Check the executable path used by the editor, especially for remote sessions:
it may differ from the server on your terminal's `PATH`. After installing a
new build, restart the language server. The LSP `initialize` response includes
the package version in `serverInfo`; it does not distinguish local commits.

## Check the logs

sail-lsp sends project-loading messages through LSP `window/logMessage`.
Inspect these in the editor's language-server logs (including Zed's RPC log).
Tracing output goes to stderr; its filter is controlled by `SAIL_LOG`:

```bash
# Set log level via environment variable
SAIL_LOG=info sail-lsp

# Or via VS Code settings
"sail-lsp.server.extraEnv": { "SAIL_LOG": "info" }
```

Use `SAIL_LOG=debug` for additional tracing. To inspect protocol messages,
enable the editor's LSP trace; that is separate from the stderr filter.

## Standard library not found

sail-lsp embeds the Sail standard library. If you see missing-prelude errors,
the embedded copy may be incompatible with your Sail project. Override with:

```bash
export SAIL_DIR=/path/to/sail   # points to the sail compiler repo root
```

When set, `$SAIL_DIR/lib/` is used instead of the embedded copy.

## Every file reports `unlinked-file`

Opening a repository directory should provide a project root to the server.
Check `rootUri` and `workspaceFolders` in the `initialize` request, followed
by the server's loading messages. For sail-riscv, expect paths like:

```
Workspace roots from initialize: file:///path/to/sail-riscv
Loaded Sail project: /path/to/sail-riscv/model/riscv.sail_project
background scan: ...
```

An empty `workspaceFolders` list falls back to a supplied `rootUri`. The server
also accepts folders added through `workspace/didChangeWorkspaceFolders` after
initialization. This is important for clients that attach roots later.

If the root is correct but no project loads, inspect the load error (for
example, ambiguous manifests or an unknown project variable). Discovery checks the
root and its immediate children; deeper projects require `workspace.projectFile`.
If a manifest loads but a file remains unlinked, check that the selected
modules/include branches reach it and that the document URI matches the loaded
path. Do not hide `unlinked-file` to diagnose a loading failure: disabling the
warning does not enable type checking.

## Project loading errors

Project failures are sent through `window/showMessage` as well as
`window/logMessage` and server status. The notification names the manifest,
the failing path or configuration, and whether analysis is incomplete or still
uses the previous project model. Identical failures are not repeatedly shown
during reloads.

* A missing source listed in a valid manifest does not discard the entire
  project. Available files remain linked in manifest order. The error lists
  the missing paths and their manifest; analysis may produce further errors
  where declarations from those files are needed. Missing paths stay watched,
  so restoring a file triggers another load.
* An invalid manifest, unknown project variable/module, or nonexistent
  `workspace.projectFile` prevents constructing a new project model. An
  existing model is retained. On initial failure, files remain unlinked and
  their `unlinked-file` diagnostics also include the actual loading error.
* Correcting the manifest or changing workspace settings triggers a reload.
  A successful load clears the failure from status and diagnostics and logs
  `Sail project loading recovered.`

`Loaded Sail project` and `ready` indicate that loading finished; check for
source file errors as well before treating the analysis as complete.

## Batch analysis

To check whether a problem is in the LSP transport or in the analysis engine,
bypass LSP and run batch analysis:

```bash
# Full corpus check (parse + type check all files)
cargo run --example corpus_check -- /path/to/project

# Performance metrics
cargo run --release --example analysis_stats -- /path/to/project
```

If `corpus_check` reports errors but the upstream Sail compiler accepts the
code, please file an issue with a minimal reproduction.

## Crash isolation

If sail-lsp crashes on a specific file, the crash is caught and reported as
a diagnostic. Other files continue working. To investigate:

```bash
# Run with backtrace
RUST_BACKTRACE=1 sail-lsp
```

## Filing issues

When filing an issue, include:

1. The editor's server executable path and installed release or source commit
2. The `.sail` file (or minimal reproduction) that triggers the problem
3. The error/warning message from the editor
4. The initialization roots and project-loading log messages; optionally stderr with `SAIL_LOG=info`

An ideal reproduction:

```bash
git clone https://github.com/user/repo.git && cd repo && git checkout <hash>
cargo run --example corpus_check -- .
```
