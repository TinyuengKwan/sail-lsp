<!---
lsp/ext.rs hash: 4740a6f4122508d3

If you need to change the above hash to make the test
`lsp_extensions_doc_is_current_with_ext_rs` pass, please check whether this
document needs adjusting as well: it describes `crates/sail-lsp/src/lsp/ext.rs`,
and the hash is of that file's text.
--->

# LSP Extensions

This document describes the requests and notifications sail-lsp speaks beyond
the Language Server Protocol. It follows rust-analyzer's
[lsp-extensions.md](https://github.com/rust-lang/rust-analyzer/blob/master/docs/book/src/contributing/lsp-extensions.md):
when in doubt, the source is `crates/sail-lsp/src/lsp/ext.rs` (the types),
`crates/sail-lsp/src/main_loop.rs` (the dispatch) and
`editors/code/src/` (the VS Code client).

Methods rust-analyzer also has keep its names and live under the
`experimental/` namespace, so a client written for rust-analyzer's shape can
reuse it; methods specific to this server are under `sail-lsp/`.

A client discovers a request through the `experimental` section of the
server's `ServerCapabilities`, and declares what it can take through the
`experimental` section of its `ClientCapabilities`. A method with no flag
listed is always available.

**Experimental server capabilities** (`capabilities.rs`):

```json
{
    "ssr": true, "ssrSearch": true, "ssrPreview": true,
    "ssrKinds": ["expression", "type", "item", "pattern"],
    "includeContexts": true, "diagnosticReport": true,
    "runnables": { "kinds": ["sail-check", "shell"] },
    "matchingBrace": true, "joinLines": true, "onEnter": true, "moveItem": true
}
```

**Experimental client capabilities** the server reads:

| Flag | Enables |
|------|---------|
| `snippetTextEdit: boolean` | `SnippetTextEdit`s in code actions, `experimental/onEnter`, `experimental/moveItem` and rename edits |
| `serverStatusNotification: boolean` | `sail-lsp/serverStatus` |
| `hoverActions: boolean` | the `actions` field on a hover result |
| `commands: { commands: string[] }` | the client-side commands a code lens may run; `sail-lsp.showReferences` is the one the server knows |

The VS Code extension declares all four (`editors/code/src/client.ts`,
`EXPERIMENTAL_CAPABILITIES`).

Every request below is handled on the analysis thread pool unless it says
*main loop*: those read or change server state that has no snapshot and
answer at once.

## Configuration in `initializationOptions`

As in rust-analyzer, `initializationOptions` may carry the `sail-lsp`
settings section, read before the server can ask `workspace/configuration`.
Sending nothing, `null` or `{}` gives the defaults. The settings are listed
in [configuration.md](configuration.md).

## Snippet `TextEdit`

**Experimental Client Capability:** `{ "snippetTextEdit": boolean }`

With this flag set, the `WorkspaceEdit` of a `textDocument/codeAction` or
`codeAction/resolve` result may contain `SnippetTextEdit`s, and
`experimental/onEnter` and `experimental/moveItem` answer with them. The
shape is rust-analyzer's, verbatim:

```typescript
interface SnippetTextEdit extends TextEdit {
    insertTextFormat?: InsertTextFormat;
}

interface SnippetWorkspaceEdit {
    documentChanges?: DocumentChanges;
    changes?: { [uri: string]: SnippetTextEdit[] };
    changeAnnotations?: { [id: string]: ChangeAnnotation };
}
```

`$0` marks where the cursor lands and `${1:name}` something to type over. A
client without the flag gets the placeholders stripped and plain edits.

`textDocument/codeAction` and `codeAction/resolve` are therefore declared in
`ext.rs` as `CodeActionRequest` and `CodeActionResolveRequest`, with a
`CodeAction` whose `edit` is a `SnippetWorkspaceEdit`; otherwise they are the
protocol's requests.

**Editors:** VS Code applies snippet edits interactively in the active editor
(`client.ts`).

## Server Status

**Method:** `sail-lsp/serverStatus` — notification, server → client

**Experimental Client Capability:** `{ "serverStatusNotification": boolean }`

```typescript
interface ServerStatusParams {
    /// `ok` | `warning` | `error`
    health: "ok" | "warning" | "error";
    /// Nothing is in flight: no scan, no configuration fetch, no cache priming.
    quiescent: boolean;
    /// Why the health is not `ok`, for a tooltip.
    message?: string;
}
```

Sent whenever the status changes — while the workspace loads
(`Loading workspace...`), when no folder or no project is configured, when
no Sail file was found, when the `$include` graph has a cycle, or when a
compiler check or the file watcher fails — and only to a client that declared
the flag. The VS Code extension shows it in the status bar (`main.ts`).

## Project Graph Changed

**Method:** `sail-lsp/projectGraphChanged` — notification, server → client,
no parameters

Emitted after a new project model is installed, which includes a
configuration reload. A client that shows the graph asks for it again
(`sail-lsp/projectGraph`) and discards older replies. VS Code's project-graph
view does (`main.ts`, `projectGraph.ts`).

## Project Graph

**Method:** `sail-lsp/projectGraph` — request, no parameters, *main loop*

**Response:** an array, the preferred project first:

```typescript
interface ProjectGraphEntry {
    manifest: DocumentUri | null;
    path: string;
    /// The project `workspace.projectFile` selects, or the first by path.
    preferred: boolean;
    /// `workspace.projectVariables` as loaded.
    variables: { [name: string]: string };
    /// `workspace.projectModules`: the module names analysis starts from.
    roots: string[];
    modules: { name: string; active: boolean; requires: string[]; files: DocumentUri[] }[];
}
```

The resolved module graph and build configuration of every loaded project;
also what a client offers when a source file belongs to several and the user
must pick an analysis context. **Editors:** VS Code (`main.ts`), the Neovim
template (`editors/templates/neovim.lua`, as a quickfix list).

## Include Contexts

**Method:** `sail-lsp/includeContexts` — request, *main loop*

**Experimental Server Capability:** `{ "includeContexts": boolean }`

**Request:** `TextDocumentIdentifier`

**Response:** one entry per instance of the file — a file reached through
`$include` from several places is analysed once per place:

```typescript
interface IncludeContext {
    /// Stable identity: root, occurrence, include chain and module.
    key: string;
    root: DocumentUri;
    includes: number[];
    project: ProjectGraphEntry | null;
    /// The context the client selected for this document, if any.
    selected: boolean;
}
```

## Select Include Context

**Method:** `sail-lsp/selectIncludeContext` — request, *main loop*

```typescript
interface SelectIncludeContextParams {
    textDocument: TextDocumentIdentifier;
    /// A `key` from `sail-lsp/includeContexts`; `null` returns to the default.
    context: string | null;
}
```

**Response:** `null`. The document must be open; a key that is no longer an
active context is refused with `InvalidParams` (`Include context is no longer
active; select it again`). Diagnostics, hover and navigation for the document
then come from the selected instance. **Editors:** VS Code offers the pick
from the status bar (`main.ts`).

## Explain Analysis

**Method:** `sail-lsp/explainAnalysis` — request

**Request:** `TextDocumentPositionParams`

**Response:** a JSON object, or `null` for a file the server does not know:
one entry per include context (`contexts`, each carrying its `project`, whether
the position is in an active preprocessor region, the name under the cursor,
whether it resolved and the candidate declarations with the reason each was
or was not chosen), plus `projectLoadError`, `reloadPending` and
`unsavedManifestPolicy`. It is the answer to "why does this name resolve
there": VS Code shows it in a panel (`main.ts`).

## Analyzer Status

**Method:** `sail-lsp/analyzerStatus` — request, no parameters

**Response:** `string`, human-readable: version, executable, workspace
roots and manifests, file counts, index sizes, include-graph statistics and
memory. rust-analyzer's `rust-analyzer/analyzerStatus`. **Editors:** VS Code
(`main.ts`).

## Diagnostic Report

**Method:** `sail-lsp/diagnosticReport` — request

**Experimental Server Capability:** `{ "diagnosticReport": boolean }`

```typescript
interface DiagnosticReportParams {
    /// The file to describe the analysis contexts of; optional.
    textDocument?: TextDocumentIdentifier;
}
```

**Response:** a JSON object with `schemaVersion: 1`, `build`, `executable`,
`platform`, `serverStatus`, `nativeAnalysis`, `compilerChecks`, `queryCache`,
`processRssBytes`, `fileWatching`, `workspace` (roots, loaded projects, load
error), `effectiveConfiguration`, `folderConfigurations`, `activeFile` (its
version and include contexts) and `analyzerStatus`. It carries no document
contents and no environment values, so it can be attached to a bug report.
**Editors:** VS Code writes it to a file (`diagnosticReport.ts`).

## View Syntax Tree

**Method:** `sail-lsp/viewSyntaxTree` — request

```typescript
interface ViewSyntaxTreeParams { textDocument: TextDocumentIdentifier; }
```

**Response:** `string`, the concrete syntax tree pretty-printed.
rust-analyzer's `rust-analyzer/viewSyntaxTree`. **Editors:** VS Code
(`main.ts`).

## View HIR

**Method:** `sail-lsp/viewHir` — request

```typescript
interface ViewHirParams { textDocument: TextDocumentIdentifier; position: Position; }
```

**Response:** `string`, the HIR expression under the cursor and its inferred
type. rust-analyzer's `rust-analyzer/viewHir`. **Editors:** VS Code (`main.ts`).

## View Item Tree

**Method:** `sail-lsp/viewItemTree` — request

```typescript
interface ViewItemTreeParams { textDocument: TextDocumentIdentifier; }
```

**Response:** `string`, the file's item tree — signatures, kinds, doc
comments — which is the surface other files see of it. rust-analyzer's
`rust-analyzer/viewItemTree`. **Editors:** VS Code (`main.ts`).

## View Include Graph

**Method:** `sail-lsp/viewIncludeGraph` — request

```typescript
interface ViewIncludeGraphParams { textDocument: TextDocumentIdentifier; }
```

**Response:** `string`, the `$include` dependency graph around the file as
readable text. Sail-specific. **Editors:** VS Code (`main.ts`).

## Expand Include

**Method:** `sail-lsp/expandInclude` — request

```typescript
interface ExpandIncludeParams { textDocument: TextDocumentIdentifier; }

interface ExpandIncludeResult {
    root: string;
    /// The file with its `$include`s expanded, transitively.
    expansion: string;
    /// The include context expanded in, when the file has several.
    context: string | null;
    project: ProjectGraphEntry | null;
    /// Where each segment of `expansion` came from.
    mappings: { range: Range; origin: Location; kind: string }[];
}
```

**Response:** `ExpandIncludeResult | null`. Sail's counterpart of
rust-analyzer's `rust-analyzer/expandMacro`: `$include` is textual, so the
expansion is what the type checker sees. **Editors:** VS Code shows it in a
virtual document and follows `mappings` back to the sources (`main.ts`).

## Open Project Manifest

**Method:** `sail-lsp/openProjectManifest` — request

```typescript
interface ProjectNavigationParams { textDocument: TextDocumentIdentifier; }
```

**Response:** `Location | null` — the manifest of the project the file belongs
to, at the `files` entry listing it (at the start of the manifest when the
file is reached through `$include`); `null` outside every project.
rust-analyzer's `openCargoToml`. **Editors:** VS Code (`main.ts`).

## Parent Module

**Method:** `sail-lsp/parentModule` — request

**Request:** `ProjectNavigationParams`

**Response:** `Location[]` — what brings the file into the project: each
manifest module whose `files` lists it, and each `$include` that names it.
rust-analyzer's `parentModule`. **Editors:** VS Code (`main.ts`).

## Matching Brace

**Method:** `experimental/matchingBrace` — request, *main loop*

**Experimental Server Capability:** `{ "matchingBrace": boolean }`

```typescript
interface MatchingBraceParams {
    textDocument: TextDocumentIdentifier;
    /// One per cursor.
    positions: Position[];
}
```

**Response:** `Position[]`, one for one with the request: the bracket matching
the one at each position, or the position unchanged when it is not on a
bracket. rust-analyzer's shape. **Editors:** VS Code (`editing.ts`).

## Join Lines

**Method:** `experimental/joinLines` — request, *main loop*

**Experimental Server Capability:** `{ "joinLines": boolean }`

```typescript
interface JoinLinesParams { textDocument: TextDocumentIdentifier; ranges: Range[]; }
```

**Response:** `TextEdit[]` joining the lines of each range the way Sail reads:
a trailing `;` or `,` and comment markers are handled, not just whitespace.
rust-analyzer's shape. **Editors:** VS Code (`editing.ts`).

## On Enter

**Method:** `experimental/onEnter` — request, *main loop*

**Experimental Server Capability:** `{ "onEnter": boolean }`

**Request:** `TextDocumentPositionParams`

**Response:** `SnippetTextEdit[] | null` — what to insert after Enter:
continue a doc comment, keep the indentation of the block just opened.

Unlike rust-analyzer's, this is asked *after* the newline is in the buffer,
with the cursor on the fresh line — the same question the `"\n"` on-type
formatting trigger asks, answered by the same code. A client written against
rust-analyzer's before-the-newline shape gets `null`. **Editors:** VS Code
(`editing.ts`).

## Move Item

**Method:** `experimental/moveItem` — request

**Experimental Server Capability:** `{ "moveItem": boolean }`

```typescript
interface MoveItemParams {
    direction: "Up" | "Down";
    textDocument: TextDocumentIdentifier;
    range: Range;
}
```

**Response:** `SnippetTextEdit[]` swapping the definition, match arm or block
item under the cursor with its sibling above or below. rust-analyzer's shape.
**Editors:** VS Code (`editing.ts`).

## Explain Formatting

**Method:** `sail-lsp/explainFormatting` — request

```typescript
interface ExplainFormattingParams {
    textDocument: TextDocumentIdentifier;
    options?: FormattingOptions;
    range?: Range;
}
```

**Response:** a JSON object: `status` (what formatting the file or range
would do, or `unavailable`), `details`, `configFile` (the `.sailfmt.toml`
that applies), `styleVersion` and the resolved `options` (`lineWidth`,
`tabSize`, `insertSpaces`). It explains a format result without applying it.
**Editors:** VS Code (`main.ts`).

## Structural Search Replace (SSR)

**Method:** `experimental/ssr` — request

**Experimental Server Capability:** `{ "ssr": boolean, "ssrKinds": string[] }`

```typescript
interface SsrParams {
    /// `pattern ==>> replacement`, with `$name` placeholders.
    query: string;
    /// Check the query only; the edit comes back empty.
    parseOnly: boolean;
    /// Flattened `TextDocumentPositionParams`: the file and position whose
    /// scope the query's names resolve in.
    textDocument: TextDocumentIdentifier;
    position: Position;
    /// Non-empty: edit only inside these ranges of that file. Empty: the
    /// project's sources.
    selections: Range[];
}
```

**Response:** `WorkspaceEdit`. `ssrKinds` names what a placeholder may stand
for. rust-analyzer's shape; the resolution is semantic, so `$x` matches an
expression and not a spelling. **Editors:** VS Code (`ssr.ts`).

## SSR Search

**Method:** `sail-lsp/ssrSearch` — request

**Experimental Server Capability:** `{ "ssrSearch": boolean }`

```typescript
interface SsrSearchParams {
    /// A pattern alone, without `==>>`.
    query: string;
    textDocument: TextDocumentIdentifier;
    position: Position;
    selections: Range[];
}
```

**Response:** `Location[]`, every match, read-only. **Editors:** VS Code
(`main.ts`, as a search result list).

## SSR Preview

**Method:** `sail-lsp/ssrPreview` — request

**Experimental Server Capability:** `{ "ssrPreview": boolean }`

**Request:** `SsrParams`

**Response:** `{ edit: WorkspaceEdit, documents: { uri, version, text }[] }` —
the edit `experimental/ssr` would return, with the exact source text each
edited document was computed against, closed files included, so a preview
can be rendered without the client reading the files itself. **Editors:**
VS Code (`ssr.ts`).

## Runnables

**Method:** `experimental/runnables` — request

**Experimental Server Capability:** `{ "runnables": { "kinds": string[] } }`

```typescript
interface RunnablesParams {
    textDocument: TextDocumentIdentifier;
    position?: Position;
}

interface Runnable {
    id: string;
    label: string;
    /// The project the runnable belongs to (its manifest).
    project: TextDocumentIdentifier;
    /// Changes when the runnable's definition changes; a stale one is
    /// re-discovered before it runs.
    fingerprint: string;
    /// Why it cannot run right now; do not execute a disabled entry.
    disabled?: string;
    documents: DocumentUri[];
    /// Where the entry belongs in the source — rust-analyzer's
    /// `Runnable::location`: a `$[test]` function or `main`. A project
    /// task has none and is shown at the top of the file.
    location?: Location;
    kind: "sail-check" | "shell";
    args: FlycheckControlParams | ProcessRunnableArgs;
}

interface ProcessRunnableArgs {
    kind: string;              // the configured task kind
    program: string;
    args: string[];
    cwd: string;
    environment: { [name: string]: string };
    /// An interpreter script the program reads from a file: `args` end
    /// with the flag that names it (`--is`), and the client stores these
    /// contents in a file and appends its path.
    script?: string;
}
```

**Response:** `Runnable[]`. rust-analyzer's discovery shape: a `sail-check`
runnable is run by sending `sail-lsp/runFlycheck` with its `args`; a `shell`
one is a process the client starts — with `script` written to a temporary
file first, since upstream Sail has no test runner and a `$[test]` function
is run by telling `sail`'s interpreter what to evaluate. The project's test
and export commands come from `runnables` in the settings
([runnables.md](runnables.md)). **Editors:** VS Code (`runnables.ts`,
`taskRunner.ts`; `location` places the entry in the Test Explorer).

## Flycheck Control

Four notifications, client → server, *main loop*, about running the `sail`
compiler beside the server's own analysis
([configuration.md](configuration.md), `flycheck`):

| Method | Params | Effect |
|--------|--------|--------|
| `sail-lsp/runFlycheck` | `FlycheckControlParams` | Start a check now |
| `sail-lsp/cancelFlycheck` | `FlycheckControlParams` | Stop the running check |
| `sail-lsp/clearFlycheck` | `FlycheckControlParams` | Drop the compiler's published diagnostics |
| `sail-lsp/recheck` | none | Re-run every check target: loaded projects and standalone open documents |

```typescript
interface FlycheckControlParams {
    /// With a document: only the loaded projects (or standalone file) that
    /// contain it. Without: every check target.
    textDocument?: TextDocumentIdentifier;
}
```

rust-analyzer's `rust-analyzer/runFlycheck`, `rust-analyzer/cancelFlycheck`
and `rust-analyzer/clearFlycheck`, with the scope widened from a document to
an optional one. **Editors:** VS Code (`main.ts`, `runnables.ts`).

### Flycheck progress

Each compiler run is a `$/progress` sequence whose token is
`flycheck/<generation>` under the `sail-lsp/` namespace, titled with the
project or file, begun
when the run is submitted, reporting `Running for Ns.` once a second, and
ended when the run finishes, is cancelled or cleared, or is superseded by a
later save. A `window/workDoneProgress/cancel` with that token cancels the
run, as rust-analyzer's `rust-analyzer/flycheck/<id>` does. Nothing is sent
to a client without `window.workDoneProgress`.

## Reload Workspace

**Method:** `sail-lsp/reloadWorkspace` — notification, client → server,
no parameters, *main loop*

Re-scans the workspace folders and rebuilds the project model, as
rust-analyzer's `rust-analyzer/reloadWorkspace`. **Editors:** VS Code
(`main.ts`).

## File Watching

Not an extension, but worth knowing: `workspace/didChangeWatchedFiles` is
registered dynamically (`client/registerCapability`) for the source roots the
project model reads — workspace folders, include search paths, the library,
the directory of every file a project reaches outside those — when a scan
starts and again when its model lands, unregistering the previous
registration first. A client with `relativePatternSupport` gets
`RelativePattern`s, others absolute globs; one without dynamic registration
is watched for by the server itself.

## Semantic Tokens

Not an extension either, but the legend is this server's own. The
`semanticTokensProvider` capability (`ext.rs::semantic_tokens_options`) is
built from the legend declared in `crates/sail-lsp/src/lsp/semantic_tokens.rs`
with `define_semantic_token_types!` / `define_semantic_token_modifiers!`, as
rust-analyzer declares its own: LSP's standard words first, then this server's.
`full`, `full/delta` and `range` are all supported.

Custom token types, each with the standard type it refines (its `superType`
in the VS Code extension's `semanticTokenTypes`, and the server's fallback):

| Type | Fallback | Meaning |
|------|----------|---------|
| `register` | `variable` | A `register`, at its declaration and every use; always `static`, `mutable` where written |
| `builtinType` | `type` | `bits`, `unit`, `bool`, ... when not shadowed by a library `type` |
| `attribute` | `decorator` | A `$[..]` attribute, as one token |
| `escapeSequence` | `string` | `\n`, `\xHH`, `\DDD` inside a string literal; the literal is cut around each |
| `punctuation` | — | Brackets, commas, semicolons, colons, dots; dropped for a standard-only client |

Custom modifiers: `controlFlow` (on `if`, `match`, `return`, ... keywords)
and `mutable` (a `var` binding at every mention; a register where it is
written). Standard modifiers sent: `declaration`, `definition`, `readonly` (a
top-level `let`), `documentation` (`///` comments), `static` (registers),
`defaultLibrary` (a name whose definition is in a library source root — the
bundled `sail-lib`).

Which classes are sent is configuration, not capability: `semanticTokens.strings`,
`.operators`, `.punctuation` (off by default) and `.nonStandardTokens`. With
`nonStandardTokens` off, each custom type goes out as its fallback — or not at
all when it has none — and the custom modifiers are cleared, so a client that
knows only LSP's legend still colours everything it can
(`to_proto::semantic_tokens`). The VS Code extension maps every type and the
`type.modifier` pairs that matter onto the TextMate grammar's scopes in
`semanticTokenScopes`, and `editors/code/src/test/semanticTokens.test.ts`
checks the legend and those contributions against each other. **Editors:**
VS Code (`package.json`).

## Cancel Request

**Method:** `$/cancelRequest` — the protocol's own notification, declared in
`ext.rs` so the main loop dispatches it like the others. Cancels the request
named; a handler that was running answers `RequestCanceled`.
