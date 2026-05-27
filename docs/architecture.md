# Architecture

This document describes the high-level architecture of sail-lsp, modeled after
[rust-analyzer's architecture guide](https://github.com/rust-analyzer/rust-analyzer/blob/master/docs/dev/architecture.md).

## Bird's Eye View

sail-lsp is a Language Server for the Sail ISA description language. It
communicates with any editor over the Language Server Protocol (LSP) via
standard I/O. The crate hierarchy mirrors rust-analyzer's layering so that
features can be ported between the two projects with minimal friction.

```
                       +---------------------+
                       |      sail-lsp       |  <- LSP transport (lib + bin)
                       |  main_loop          |
                       |  handlers/ lsp/     |
                       |  mem_docs task_pool |
                       |  reload  op_queue   |
                       +--+---+---+---+--+---+
          +---------------+   |   |   |  +----------+
          v                   v   v   v             v
    +----------+  +------------+ +----------+ +----------+  +----------+
    |   ide    |  |ide-assists | |ide-compl.| |ide-diag. |  | ide-ssr  |
    | 39 mods  |  | 51 handlers| |11 provdrs| |40 handlrs|  |ssr engine|
    +----+-----+  +------+-----+ +----+-----+ +----+-----+  +----+-----+
         |               |            |             |              |
         +-------+-------+------------+-------------+--------------+
                 v
           +----------+
           |  ide-db  |  RootDatabase, WorkspaceSymbolIndex,
           | FileDb   |  search, prime_caches, active_parameter,
           | search   |  documentation, imports, rename, fixture
           +----+-----+
                |
        +-------+----------+
        v       v          v
     +-----+ +------+ +--------+
     | hir | |hir-ty| |hir-def |
     |Sema | |infer/| |ItemTree|
     |     | |lower/| |DefMap  |
     +--+--+ +--+---+ +---+----+
        |       |          |
        +-------+----------+
                v
          +----------+
          |  syntax  |  <- rowan CST + ted tree editor
          +----+-----+
               v
          +----------+
          |  parser  |  <- hand-written lexer + SyntaxKind
          +----+-----+
               v
          +----------+
          | base-db  |  <- salsa inputs (FileText, FileId, VFS)
          +----+-----+
               |
        +------+------+
        v             v
   +--------+   +-----------+
   |  vfs   |   | vfs-notify|  <- filesystem abstraction
   +--------+   +-----------+

  +-------+  +-------+  +--------+  +---------+
  | stdx  |  | paths |  | intern |  | profile |  <- utility crates
  +-------+  +-------+  +--------+  +---------+
```

## Crate Responsibilities

### Foundation layer

| Crate | Role |
|-------|------|
| `base-db` | Salsa 0.25.2 infrastructure: `FileText` (salsa input), `FileId(u32)`, `SourceDatabase` trait, `Files` (VFS with content-hash dedup + pending change batching), `WorkspaceFiles` (salsa input consumed by `workspace_context_query`), `SourceRoot` (local vs library), `Durability` (HIGH for disk, LOW for open files). |
| `parser` | Hand-written Sail lexer (`hand_lexer.rs`), `Token`/`Span` types, `SyntaxKind` enum (~200 variants), `SailLanguage` (rowan Language trait), `Event` protocol, full-fidelity lex pipeline (`lex_full.rs`), `Literal` enum (shared by parser/hir-def/hir-ty), `TokenSet` for parser recovery. `KW_CONFIG` token, `InfixOp`/`OP_IDENT` for subscript operators like `<_s`, `<=_u`. |
| `syntax` | Rowan CST: `event_parser` (Pratt parser -> SyntaxNode), typed AST wrappers (`ast.rs`), stable pointers (`ptr.rs`: SyntaxNodePtr, AstPtr), CST->ParsedFile lowering (`cst_lower.rs`), salsa tracked queries (`parse_query.rs`: parse_file, parsed_file_query), tree editor (`ted.rs`: insert/remove/replace without re-parsing), preprocessor types (`preprocess.rs`). Parser handles: `config` expressions, scattered `clause` definitions, `private` modifier, `$[attr]` attributes, `///` doc comments, infix subscript operators, `[e with f=v]` struct update, mapping guards `if ... =>`, type ascription `(e : t)`, `try/catch`. |
| `stdx` | Generic helpers: `format_to!`, `NonEmptyVec`, `is_ci`, `anymap`, `panic_context`, `process`, `rand`, `thread` pool, `variance`. No Sail-specific code. |
| `paths` | `AbsPath`/`AbsPathBuf` newtypes for absolute filesystem paths, backed by `camino`. |
| `intern` | GC-able interning infrastructure: `intern`, `intern_slice`, `gc`, `symbol`. |
| `profile` | `StopWatch`, `MemoryUsage`, optional `google_cpu_profiler` for CPU profiling. |
| `vfs` | Virtual filesystem abstraction: `VfsPath`, `FileSet`, `AnchoredPath`, `PathInterner`, `Loader` trait. Decouples the server from the physical filesystem. |
| `vfs-notify` | `notify`-based file watcher implementing the `vfs::Loader` trait. |

### Definition layer

| Crate | Role |
|-------|------|
| `hir-def` | Per-file stable data structures: `Body`/`ExprId`/`PatId` arenas + `BodySourceMap`, `ExpressionStore` (`expr_store/`), `ItemTree` (`item_tree/` — signature-hash), `CallableBodies` + `EffectTag` (11 variants), `CallGraph`, `nameres/` (DefMap + DefId), `Resolver` (scope-chain: Local > Module > Workspace), `Name` (SmolStr-interned), `IncludeGraph`, `AnalysisScope`, scattered definition tracking, `WorkspaceDefMap`, `bitfield`, `callable_info`, `diagnostics`, `effects`, `find_path`, `signatures`, `project`. Salsa queries in `def_query`: item_tree, callable_bodies, callgraph, CallableId (interned), file_callable_ids. Database trait: `DefDatabase`. |
| `hir` | `Semantics` facade for IDE consumers: type queries, `diagnostics.rs` (`AnyDiagnostic` enum + `inference_diagnostic()` cooking), `hir_query` (workspace callgraph builder), ref/impl counts, workspace def map. `symbols.rs`, `display.rs`, `from_id.rs`, `has_source.rs`. Does NOT depend on ide-db (RA architecture). |
| `hir-ty` | Type-checker: `infer/` (multi-file inference engine), `lower/` (type lowering), `typecheck` (InferenceContext with body/resolver/return_ty/diverges), `subtype.rs` (directional type checks), `match_check` (pattern exhaustiveness), `overload.rs` (Sail multi-dispatch), `flow.rs` (control flow analysis), `representability.rs`, `inhabitedness.rs`, `query.rs` (per-callable `infer_callable_query`). Effect system: declared + inferred + transitive. Z3 SMT solver for numeric constraints. Database trait: `HirDatabase`. **LSP-free** -- no lsp-types dependency. |

### IDE layer

| Crate | Role |
|-------|------|
| `ide-db` | Foundation: `FileDb` trait, `RootDatabase` (#[salsa::db]) + `SalsaFile` adapter, `defs` (SymbolKind, CompletionItemKind), `text_edit` (TextEdit), `source_change` (SourceChange), `workspace_index` (WorkspaceSymbolIndex -- O(1) name lookup), `search` (FindUsages -- two-stage search), `active_parameter` (ActiveParameter), `prime_caches` (salsa pre-warming with parallel rayon phase), `ide_types` (HoverResult, InlayHint, Diagnostic, Annotation, HlRange, etc.), `line_index`, `symbol_index`, `helpers`, `keywords`, `pragmas`, `documentation`, `fixture`, `imports/`, `rename`, `syntax_helpers/`, `type_inference`, `span`, `text_document`. |
| `ide` | 39 IDE feature modules: `goto_definition`, `goto_declaration`, `goto_implementation`, `goto_type_definition`, `references`, `rename`, `hover/`, `syntax_highlighting/`, `annotations/`, `inlay_hints/`, `call_hierarchy`, `calls`, `signature_help`, `folding_ranges` (CST-aware), `file_structure`, `matching_brace`, `extend_selection`, `highlight_related`, `typing/` (on_enter), `join_lines`, `move_item`, `formatting`, `completion`, `ssr`, `navigation`, `navigation_target`, `analysis` (Analysis/AnalysisHost), `moniker`, `static_index`, `markup`, `doc_links/`, debug views (view_hir, view_item_tree, view_syntax_tree), Sail-specific (bitfield_layout, effect_annotations, expand_include, include_graph_view). |
| `ide-completion` | Provider-based completion: `context.rs` (CompletionContext with `sema: Semantics`, `db`, `expected_type`, `DotAccess` with `receiver_ty`), `completions.rs` (Completions accumulator + `complete_name_ref` dispatcher), `completions/` directory with 11 providers (dot, expr, flyimport, item_list, keyword, pattern, postfix, pragma, record, snippet, type_). Entry: `completions()`. |
| `ide-diagnostics` | Diagnostic rendering + 40 handlers: `DiagnosticsContext` with `sema: Semantics`, handlers import `AnyDiagnostic` from `hir::diagnostics`. Parse errors via `parse_errors()` firewall query. `err_recover` pattern. `new_with_syntax_node_ptr()` for precise ranges. **LSP-free**. |
| `ide-assists` | Code actions: `assist_context.rs` (AssistId, AssistKind, Assist, AssistContext, Assists accumulator, Handler type), `handlers/` (51 registered handlers via `all()` static array), `assists()` entry point (RA pattern). |
| `ide-ssr` | Structural search-replace: `parsing` ($name placeholder syntax), `matching` (AST pattern match), `resolving` (semantic resolution), `replacing` (substitution), `nester` (nested match handling), `fragments` (partial-tree matching), `search` (workspace-wide SSR), `errors`, `from_comment` (SSR patterns in comments). |

### Transport layer

| Crate | Role |
|-------|------|
| `sail-lsp` | LSP server with **lib + bin split**: `main.rs` (thin binary entry calling `sail_lsp::main()`), `lib.rs` (library exposing all server logic). **main_loop.rs**: `GlobalState`, event loop matching RA `handle_event` exactly: select! -> coalesce -> process_changes (vfs_done gated) -> quiescence block (diagnostics, GC) -> op queues -> take_changes -> publish -> status -> timing. **global_state.rs**: `GlobalStateSnapshot` with `include_graph` + `client_caps`. **handlers/**: `dispatch.rs` (RequestDispatcher with `on_sync`/`on` -- both `catch_unwind`), `request.rs`, `notification.rs`. **lsp/**: `capabilities.rs` (server + client capabilities), `from_proto.rs`, `to_proto.rs`, `ext.rs`. **mem_docs.rs**: MemDocs with `take_changes()`. **task_pool.rs**: rayon-backed TaskPool with `catch_unwind` at spawn level. **reload.rs**: workspace reload logic. **op_queue.rs**: typed operation queue. **diagnostics.rs**: DiagnosticCollection + fetch_native_diagnostics (catch_unwind). **config.rs**: SailLspConfig. **cli.rs**: command-line interface. **hover_ext.rs**, **code_action_helpers.rs**: handler helpers. **integrated_benchmarks.rs**: `#[cfg(test)]` benchmarks (run via `RUN_SLOW_BENCHES=1`). **tests.rs**: LSP integration tests. |

## Key Design Patterns

### lib + bin split (sail-lsp)

The `sail-lsp` crate has both `lib.rs` and `main.rs`. The binary is a thin
wrapper calling `sail_lsp::main()`. This enables `integrated_benchmarks.rs` to
live as a `#[cfg(test)]` module with full access to server internals, matching
RA's pattern for in-crate benchmarks.

### Async dispatch (RA pattern)

`RequestDispatcher` has two execution modes:
- **`on_sync`**: runs on main thread with `&GlobalStateSnapshot` (fast, for formatting/symbols)
- **`on`**: clones snapshot, spawns on rayon thread pool (for hover/goto/completion)

Responses from async handlers flow back as `Task::Response` through the unified Task enum.
Main loop never blocks on handler execution. Cancellation via `salsa::Cancelled` panic catching.

### process_changes (RA pattern)

Notification handlers buffer VFS changes to a pending queue. `process_changes()` flushes them
to salsa in a single batch **after** event handling (lazy, RA pattern). This batches
rapid-fire edits into one salsa revision bump.

### WorkspaceSymbolIndex

Built after workspace scan. HashMap-based name->SymbolEntry index. Used by:
- GotoDefinition/Declaration/Implementation (O(1) instead of O(n) file scan)
- WorkspaceSymbol search
- Reference/implementation count caching

Updated incrementally on file changes via `index_dirty_files`.

### Two-stage search (ide-db/search.rs)

RA pattern: `FindUsages` with `SearchScope`:
1. **Stage 1**: WorkspaceSymbolIndex lookup -> candidate file URLs (O(1))
2. **Stage 2**: Per-file `symbol_occurrences` scan in candidates only

Returns `UsageSearchResult { references: HashMap<Url, Vec<FileReference>> }` with
`ReferenceCategory` (Read/Write/Import).

### Cache pre-warming (ide-db/prime_caches.rs)

Two-phase pre-warming after workspace scan:

- **Phase 1** (sequential): `parse_file` -> `parsed_file_query` -> `item_tree_query` -> `callable_bodies_query` per file (~230ms).
- **Phase 2** (parallel): `infer_callable_query` for all callables via rayon `par_iter()` + `db.clone()` (~1.6s on 24 threads, 8.8x speedup vs sequential 14.2s).

Takes `&RootDatabase` (concrete type, not trait object) for `db.clone()` in rayon workers.
Cancellation checked per-callable. Reports "Indexing" and "Type inference" progress phases.

### Salsa query chain

```
FileText (salsa input)
  -> parse_file(db, ft)             -> ParsedFileData {tokens, green, errors}
  -> parse_errors(db, ft)           -> Option<Box<[SyntaxError]>>  <- firewall query
  -> parsed_file_query(db, ft)      -> ParsedFile {decls, symbol_occurrences, call_sites}
  -> item_tree_query(db, ft)        -> ItemTree {entries, signature_hash, fixities}
  -> callable_bodies_query(db, ft)  -> CallableBodies {per-callable Body arenas}
  -> callgraph_query(db, ft)        -> CallGraph {forward, reverse, sites}
  -> file_callable_ids(db, ft)      -> Vec<CallableId>
  -> infer_callable_query(db, id)   -> TypeCheckResult {inference, diagnostics}
  -> signature_index_query(db, ft)  -> HashMap<String, CallableSignature>

WorkspaceFiles (salsa input)
  -> workspace_context_query(db, wf) -> WorkspaceContext
     (reads top_level_env_query x N files)
```

Per-item inference: editing one function body only re-infers that function.
`body_with_source_map_query` has LRU=512 (matching RA).

Workspace context is a salsa tracked query (`workspace_context_query` in
`hir-ty/src/query.rs`), not a global static. It takes a `WorkspaceFiles` salsa
input and reads `top_level_env_query` for each file to build the combined
workspace environment.

### Completion provider architecture (ide-completion)

RA pattern: `CompletionContext` (with `sema: Semantics`) -> `Completions` accumulator -> providers -> `CompletionItem`.

11 providers: dot, expr, flyimport, item_list, keyword, pattern, postfix, pragma, record, snippet, type_.

All providers follow `(acc: &mut Completions, ctx: &CompletionContext, ...)` parameter order (RA convention).

### Assist handler architecture (ide-assists)

RA pattern: `AssistContext` -> `handlers::all()` -> `Assists` accumulator.

51 handlers registered in `handlers/mod.rs` via `all()` static array.

### Diagnostic handler architecture (ide-diagnostics)

40 handlers with `err_recover` pattern. `DiagnosticsContext` holds `sema: Semantics`.
Handlers import `AnyDiagnostic` from `hir::diagnostics`. Parse errors via firewall query.

### SSR engine (ide-ssr)

Full structural search-replace pipeline:
1. **Parsing**: `$name` placeholder syntax -> SSR pattern AST
2. **Matching**: pattern AST matched against code AST
3. **Resolving**: semantic resolution of matched nodes
4. **Replacing**: substitution with placeholder bindings
5. **Nesting**: handles nested match occurrences
6. **Search**: workspace-wide SSR via two-stage search

### FileDb trait (ide-db)

Every IDE feature module takes `&dyn FileDb` instead of depending on the binary
crate. Methods: `text()`, `position_at()`, `offset_at()`, `tokens()`, `token_at()`,
`parsed()`, `item_tree()`, `bodies()`, `signature_index()`, `ref_counts()`,
`impl_counts()`, `diagnostics()`.

`SalsaFile` implements `FileDb` by delegating to salsa tracked queries -- automatically
memoized and invalidated when `FileText` changes.

### ted.rs -- syntax tree editor (syntax)

Mutations without re-parsing via rowan's `splice_children`:
- `Position`: `after(elem)`, `before(elem)`, `first_child_of(node)`, `last_child_of(node)`
- Operations: `insert_raw`, `insert_all_raw`, `remove`, `remove_all`, `replace`, `replace_with_many`, `replace_all`, `append_child`, `prepend_child`
- `Element` trait abstracts over SyntaxNode/SyntaxToken/SyntaxElement

### Effect system

Tracks side effects through the type-checker and call graph:
1. **EffectTag** (hir-def): 11 variants per CallableBody
2. **Declared effects**: from TYPE_EFFECT CST nodes in function signatures
3. **Inferred effects**: transitive propagation via `transitive_effects_query` salsa query
4. **Outcome effects**: effect tracking through function outcomes
5. **Pure function enforcement**: val spec `pure` -> diagnostic on effect mismatch
6. **Hover display**: shows both declared and transitive effects
7. **Code lens**: per-callable effect annotations with propagation info

### Workspace reload

`didChangeWatchedFiles` with `.sail` file CREATED/DELETED sets `needs_reload` flag.
Main loop spawns background `Task::WorkspaceScan` to re-scan workspace folders,
rebuild include graph + SourceRoots + WorkspaceSymbolIndex. Progress reported.

### Cancellation

`dispatch.rs::catch_cancelled` wraps handlers in `std::panic::catch_unwind`.
`salsa::Cancelled` panic -> LSP `ContentModified` error response.
Async handlers on thread pool can be effectively cancelled when `on_did_change`
modifies salsa inputs (snapshot revision mismatch triggers Cancelled).
