# Architecture

Sail-LSP separates its incremental analysis engine from LSP transport, following
[rust-analyzer's architecture](https://rust-analyzer.github.io/book/contributing/architecture.html).
Native analysis runs without a Sail compiler. Optional Flycheck adds upstream
compiler diagnostics alongside native results.

## Layers and entry points

```text
sail-lsp                         LSP, configuration, VFS loading, task lifecycle
  -> ide                        Analysis snapshots and feature orchestration
  -> ide-assists / ide-completion / ide-diagnostics / ide-ssr
       -> ide-db                RootDatabase, source changes, indexes and search
            -> hir              Semantics facade, definitions and source identity
                 -> hir-ty      Inference, subtyping, effects and numeric proofs
                 -> hir-def     Items, scopes, bodies and ordered declarations
                 -> hir-expand  Preprocessing projections
                      -> syntax -> parser

base-db                         Salsa source inputs and change transactions
```

`vfs` and `vfs-notify` supply filesystem loading and watching. `project-model`
parses and evaluates project manifests. `stdx`, `paths`, `intern` and `profile`
provide shared infrastructure.

The server calls features through `ide::Analysis`; handlers convert protocol
parameters and results. `hir::Semantics` owns semantic queries used by the IDE
layer. `FileDb` provides source/syntax access and a bridge to the database for
compatibility callers. New semantic decisions belong in `Semantics` rather than
another `FileDb` adapter.

Dependencies point downwards: `hir` does not depend on `ide-db`, and IDE/type
crates do not depend on LSP types. Byte offsets remain internal until the
transport converts them through a `LineIndex` to UTF-16 positions.

## Source inputs and project identity

The VFS stores physical file contents. Editor overlays take precedence over disk
contents; closing a document restores its disk-backed state. Notifications buffer
changes, and the main loop drains them through `Change`/`FileChange` into one
Salsa input transaction. Source roots determine durability. Retained analysis
snapshots must be released before a database write.

Project discovery uses explicit configuration or a bounded search of each
workspace root and its immediate children. It does not construct a program by
recursively sorting Sail files. Manifest order, selected module dependencies,
typed variables and preprocessing determine the active program.

VFS presence and project membership are separate. An open unlinked file retains
local syntax/editing features, but project-dependent diagnostics require a loaded
semantic context. Missing project sources are reported while available files
remain linked. An invalid reload retains that project's last usable model;
failures in one workspace folder do not discard other projects.

Preprocessing records **include visits**, not only physical paths. A visit is
identified by its root occurrence, module and include chain. Multiple active
visits can share physical text while retaining different semantic file handles,
region masks and environments. Physical locations are used for editor output;
semantic identities are used for resolution and inference.

Position queries select an active view. Diagnostics aggregate views at physical
locations. Selecting an include context changes read preference, but edits still
check all affected instances. Inactive conditional branches do not contribute
semantic declarations. See [project configuration](configuration.md#workspace)
for selection and reload behavior.

## Incremental queries

The main dependency chain is:

```text
FileText
  -> cached parse / validation
  -> ItemTree and ordered declaration projections
  -> callable bodies and source maps
  -> selected declaration environment
  -> per-callable inference
  -> IDE feature results

WorkspaceFiles + preprocessing configuration
  -> ordered source/include plan
  -> membership, visibility and declaration directory
```

The parser keeps a previous-tree seed for safe local token/block reparsing and
falls back to a full parse for unsafe edits. Full-fidelity tokens, validation and
compatibility metadata are computed lazily from the exact source snapshot.
Cancellation must propagate through parser recovery rather than becoming a
synthetic syntax error.

Item and declaration projections separate semantic content from positions.
Body-only edits can reuse unrelated signatures and inference. Trivia-only edits
can reuse semantic bodies while source maps reflect current offsets. Bodies and
source maps are shared separately through `Arc`; result equality compares values,
not allocation identities.

Configured projects use ordered declaration contributions. Each callable selects
the signatures, types, fields, overloads and constraints reachable from its roots.
Missing-name lookups depend on the declaration directory, so adding a previously
missing declaration invalidates consumers. Local declarations retain their
precedence, and later declarations cannot become visible through a fallback.

Selection is conservative: shared fields, recursive types, global constraints
and unresolved scope edges can broaden dependencies. Revalidating cached queries
is distinct from re-executing inference. Incrementality tests check both Salsa
executions and result reuse; historical timings do not establish RA performance
parity.

Cache limits and memory-pressure collection operate at idle boundaries with
exclusive database access. They cannot force release of trees or solver contexts
still owned by snapshots. See [cache policy](configuration.md#analysis-cache-policy)
and [validation tools](../CONTRIBUTING.md#validation).

## Type checking and proofs

`hir-ty` keeps type, integer, boolean and order arguments distinct. Subtyping is
directional and transactional. Compatibility separates proven relations, deferred
obligations, recovery and mismatches; acceptance for continued inference is not a
proof. Speculative overload checks roll back bindings, fresh variables, captured
obligations and placeholder mappings together.

Substitution is simultaneous and capture-avoiding across aliases, existential
binders and quantifiers. Numeric refinements and branch assumptions are lexically
scoped. Obligations retain their source, cause and assumptions until finalization,
before unresolved placeholders are erased for display. General branch joins and
flow-sensitive refinement remain incomplete.

Bounded native arithmetic handles cheap proofs before the optional Z3 fallback.
Partial arithmetic keeps its domain requirements. Timeout, cancellation and
resource exhaustion produce Unknown, never evidence that a constraint is false.
Unresolved reportable constraints retain their warning policy; ordinary recovery
does not create an invented error.

`Analysis::with_db` establishes a solver budget shared by nested queries. Direct
inference installs bounded defaults as well. Scoped watchdogs interrupt active
solver checks and finish before their contexts are dropped. Resource-limited
results retry after a later database revision; normal results retain tracked
dependencies. Numeric proofs, subtype rules and supported compiler modes have
separate regression/oracle coverage.

Effects combine declared annotations, body inference and transitive call-graph
information. IDE presentation distinguishes source-written schemes from confirmed
call-site specializations. Ambiguous overloads and unproved applications do not
publish a fabricated specialization.

## IDE features and source edits

Completion combines cursor classification, visible declarations and expected-type
ranking. A temporary reparsed identifier classifies incomplete syntax without
changing database inputs or source coordinates. Recovery types do not count as
proven matches. Type-directed search uses bounded work and validates the complete
application before offering a candidate.

References and rename share semantic definition identity. Search first restricts
scope and text candidates, then checks syntax and binding identity. Comments,
strings, shadowed locals and unrelated fields cannot become usages by spelling
alone. Rename and signature edits validate bindings across affected source views;
unsafe or unsupported edits are declined.

Assists return `SourceChange`, including resource operations separately from text
edits. The server checks client capabilities, source versions and current edit
preconditions when resolving actions. Inlining and parameter removal preserve
evaluation order and effects; real-compiler execution tests cover supported cases.
Formatting shares one engine between editor requests and `sail-lsp fmt`; its
project configuration and no-op reasons are documented in
[formatting](configuration.md#formatting).

### Structural search and replacement

SSR supports expression rules by default, with `type:`, `item:` and `pattern:`
fragment grammars. Fixed names resolve in the invocation scope; captures retain
source correspondence and precedence. Rendering protects binding identity and
freshens introduced binders. Replacements that discard comments are skipped, and
the resulting file must parse.

`sail-lsp/ssrSearch` returns physical, deduplicated locations.
`experimental/ssr` produces versioned workspace edits; `sail-lsp/ssrPreview` adds
source snapshots for client previews. Apply rechecks source versions, and edits
or closes invalidate the preview. Read selection may disambiguate an include
instance, while edits retain all-instance safety checks. Library files are
excluded. SSR checks syntax and binding identity, not behavioral equivalence or
all dependent type obligations.

## Server lifecycle and external tools

The event loop batches input changes and schedules loading, diagnostics and cache
work. Semantic requests run on workers with immutable snapshots; execution
priority and cancellation retry policy are separate. A cancelled/completed
request cannot later publish a response or retry.

Large-project preprocessing can run on an independent worker database. Results
carry immutable inputs and are installed only if generation, configuration and
overlay contents still match. At most one worker and one replacement input are
retained. Watcher recovery reloads known dependencies while preserving unsaved
buffers. Stale scan, task and diagnostic results are discarded.

Native diagnostics and per-project compiler results have separate lifecycles.
Flycheck checks saved inputs with the owning project's configuration, and defers
while affected buffers are dirty. Unsupported compiler configurations are visible
skips rather than checks against different defaults. Optional context and JSON
diagnostic protocols are supplied as companion Sail patches; ordinary checks use
the stock CLI. See [Flycheck](configuration.md#flycheck) for protocol versions,
patches, fallback behavior, cache policy and limitations.

Project tasks use explicit argument arrays and a versioned project context.
Discovery does not execute commands. Clients revalidate tasks and dirty inputs
before launch, and cancellation/shutdown reap their processes. See
[project tasks](runnables.md) for the client contract and adapters.

## Verification and maintenance

[CONTRIBUTING.md](../CONTRIBUTING.md) owns build commands, regression tools and
oracle regeneration. [Performance gates](performance-gates.md) distinguish
same-host regression comparisons from pinned real-model targets.
[Upstream monitoring](upstream-monitoring.md) covers manifest semantics and
compiler/model revisions. These scopes are independent: passing typecheck
fixtures does not establish project-system or editor-protocol equivalence.

Keep run-specific reports, raw measurements and temporary probes in ignored
`target/` or CI artifacts. Maintained tests, policies and oracle inputs belong in
the repository; guides describe lasting behavior and limits.
