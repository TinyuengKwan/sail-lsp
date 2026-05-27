# CLAUDE.md

A multi-crate Rust workspace implementing the **Sail** language server, structured to mirror rust-analyzer's crate layout with salsa-backed incremental queries, rowan lossless CST, and lsp-server async dispatch. Full RA architectural alignment completed.

## Build & test

```bash
cargo build --workspace                                # debug build
cargo test --workspace                                 # all tests
cargo build --release                                  # release binary: target/release/sail-lsp

# Integrated benchmarks (requires sail-riscv checkout)
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks

# Corpus validation against sail-riscv (158 files)
cargo run --example corpus_check -- /path/to/sail-riscv
cargo run --example corpus_check -- --workspace /path/to/sail-riscv

# Performance benchmark (mirrors RA analysis-stats)
cargo run --release --example analysis_stats -- /path/to/sail-riscv
cargo run --release --example analysis_stats -- -v /path/to/sail-riscv
```

## Corpus quality (sail-riscv, 158 files)

| Category | Initial | Current | Reduction |
|----------|---------|---------|-----------|
| Parse errors | 140 | **0** | **-100%** |
| NameRes issues | 414 | **0** | **-100%** |
| Type errors | 736 | **0** | **-100%** |
| Unused-var FP | — | **0** | — |
| Crashes | 6 | **0** | **-100%** |
| **Total** | **1,296** | **0** | **-100%** |

Remaining diagnostics (all true positives, zero false positives):
- 70 unused-variable (genuinely unused let bindings)
- 2 redundant-type-annotation (code style suggestion)
- 2 unreachable-code (code style suggestion)

Compiler warnings: **0** (`cargo build --workspace` is warning-free).

## CI

CI runs at `.github/workflows/ci.yaml`: Rust build+test (3 OS via nextest), rustfmt,
clippy (`dbg_macro`, `todo`, `print_stdout`, `print_stderr` denied), VS Code extension
(typecheck + build + package), typos, and a conclusion gate job.

## VS Code extension

A dedicated VS Code extension lives at `editors/code/` with TextMate grammar,
language configuration, and a TypeScript LSP client.

## Crate hierarchy (21 crates, resolver = "2")

```
crates/
  stdx/             — format_to!, NonEmptyVec, is_ci, anymap, panic_context,
                      process, rand, thread pool, variance (9 modules)
  paths/            — AbsPath, AbsPathBuf (camino-backed)
  intern/           — interning infrastructure (gc, intern, intern_slice, symbol)
  profile/          — StopWatch, MemoryUsage, google_cpu_profiler
  vfs/              — VFS: VfsPath, FileSet, AnchoredPath, PathInterner, Loader trait
  vfs-notify/       — notify-based file watcher implementing vfs::Loader
  base-db/          — salsa 0.25.2 inputs: FileText, FileId, Durability,
                      SourceDatabase, SourceRoot, VFS (Files + content-hash dedup),
                      WorkspaceFiles (salsa input for workspace context query)
  parser/           — hand-written Sail lexer (hand_lexer.rs), SyntaxKind enum,
                      rowan Language trait, Event protocol, Token/Span types,
                      full-fidelity lex pipeline (lex_full.rs), Literal enum,
                      KW_CONFIG (config keyword), InfixOp/OP_IDENT (<_s, <=_u etc.)
  syntax/           — event_parser (rowan CST), ast (typed AST wrappers),
                      ptr (SyntaxNodePtr, AstPtr), cst_lower (CST->ParsedFile),
                      parse_query (salsa tracked: parse_file, parsed_file),
                      ted (tree editor: insert/remove/replace),
                      preprocess (default_symbols, PreprocessOptions)
  project-model/    — ALIGN(ra): .sail_project file parser (RA equivalent: Cargo.toml
                      / rust-project.json). Standalone crate, no hir dependencies.
  hir-expand/       — ALIGN(ra): mirrors RA's hir-expand crate. Handles $include
                      graph resolution (RA handles macro expansion):
                      IncludeGraph, AnalysisScope, InFile<T>, ExpandDatabase trait,
                      include_paths/typed_include_paths salsa queries,
                      WorkspaceIncludeGraph (salsa singleton)
  hir-def/          — Body (embeds ExpressionStore), ExprCollector (CST->HIR lowering),
                      ItemTree (typed arenas via ModItemId + per-kind Arena),
                      nameres (DefMap with ItemScope), DefCollector,
                      Resolver (scope-chain: Local > Module > Workspace,
                        methods: names_in_scope, item_scope, scopes),
                      per_ns (PerNs + Namespace), item_scope (ItemScope),
                      Name (SmolStr-interned), CallableBodies, EffectTag,
                      scattered, workspace_def_map,
                      expr_store (Arena<Expr> + Arena<Pat>),
                      def_query (salsa: file_item_tree + callable_bodies +
                        callgraph + DefWithBodyId + body_with_source_map),
                      ast_id, bitfield, callable_info, diagnostics,
                      effects, find_path, signatures
                      Re-exports from hir-expand: include_graph, analysis_scope,
                      in_file, project (backward compat)
  hir/              — Semantics facade (s2d_cache, type/name/resolve via SourceAnalyzer),
                      SourceAnalyzer (resolver + BodyOrSig + resolve_field/resolve_method_call),
                      public HIR types: Adt (alias TypeDef), GenericParam (aliases TypeVar/TypeParam),
                      Function, PathResolution, Local, Definition,
                      diagnostics (AnyDiagnostic), hir_query (workspace callgraph),
                      classify_name_ref stub, db.rs, symbols.rs, display.rs, from_id.rs
                      NOTE: hir does NOT depend on ide-db (RA architecture)
  hir-ty/           — typecheck (InferenceContext<'db> with db/owner/resolver/return_ty/diverges),
                      InferenceResult (ArenaMap<ExprId,Ty> + TypeMismatch + InferenceDiagnostic,
                        method_resolutions: (FunctionId,FileId)),
                      TypeCheckResult = InferenceResult (type alias),
                      Ty/TyKind (Arc-interned, pub kind()), Diverges enum,
                      infer/ (multi-file inference engine), lower/ (type lowering),
                      method_resolution (Candidate, Pick, MethodResolutionContext),
                      match_check, effect system (is_pure_context + observed_effects),
                      flow analysis, representability,
                      Z3 solver, query.rs (salsa tracked: infer, infer_body,
                        infer_for_body, top_level_env, workspace_context, transitive_effects)
  ide-db/           — FileDb trait, RootDatabase + SalsaFile, line_index,
                      defs (Definition with ALIGN/CUSTOM annotations, SymbolKind),
                      text_edit (TextEdit), source_change (SourceChange),
                      workspace_index (SymbolIndex — renamed from WorkspaceSymbolIndex),
                      search (FindUsages, two-stage search),
                      active_parameter (ActiveParameter),
                      prime_caches (salsa cache pre-warming),
                      ide_types (HoverResult, InlayHint, Diagnostic, etc.),
                      symbol_index, helpers, keywords, pragmas,
                      documentation, fixture, imports, rename,
                      syntax_helpers, type_inference, span
  ide/              — 39 modules: goto_*, references, rename, hover/,
                      syntax_highlighting/, annotations/, inlay_hints/,
                      formatting, completion, ssr, navigation, navigation_target,
                      call_hierarchy, calls, signature_help, folding_ranges,
                      file_structure, matching_brace, extend_selection,
                      highlight_related, typing/, join_lines, move_item,
                      analysis (Analysis/AnalysisHost), moniker, static_index,
                      markup, doc_links/,
                      view_hir, view_item_tree, view_syntax_tree,
                      bitfield_layout, effect_annotations, expand_include,
                      include_graph_view
  ide-completion/   — context (CompletionContext), provider architecture:
                      completions/ (keyword, expr, dot, pattern, item_list,
                      flyimport, postfix, pragma, record, snippet, type_)
  ide-diagnostics/  — 40 diagnostic handlers, parse/semantic/type_error checkers,
                      effect mismatch diagnostics [LSP-FREE]
  ide-assists/      — assist_context (Assist, AssistContext, Assists accumulator),
                      handlers/ (51 registered handlers via all()),
                      assists() entry point (RA pattern)
  ide-ssr/          — structural search-replace with $name placeholders,
                      parsing, matching, resolving, replacing, nesting,
                      fragments, search, errors
  sail-lsp/         — LSP server (lib + bin split):
                      main.rs (binary entry), lib.rs (library entry),
                      main_loop (GlobalState, event loop, on_request/on_task),
                      global_state (GlobalStateSnapshot),
                      handlers/ (dispatch, request, notification),
                      lsp/ (capabilities, from_proto, to_proto, ext),
                      mem_docs (MemDocs, DocumentData),
                      task_pool (TaskPool, Task enum with Response variant),
                      diagnostics, progress, config, reload, op_queue,
                      cli (command-line interface),
                      hover_ext, code_action_helpers,
                      integrated_benchmarks, tests
```

## Architecture

```
sail-lsp (lib + bin, lsp-server async dispatch, lsp-types confined here)
  |
  +-- main.rs (binary entry: calls sail_lsp::main())
  +-- lib.rs (library: GlobalState, main_loop, handlers, etc.)
  +-- lsp/ (capabilities, to_proto, from_proto, ext)
  +-- handlers/ (dispatch, request, notification)
  +-- main_loop (GlobalState, event loop, on_request/on_task/on_notification)
  +-- global_state (GlobalStateSnapshot for handler isolation)
  +-- mem_docs (MemDocs — open document tracking)
  +-- task_pool (TaskPool + Task::WorkspaceScan/Response)
  +-- diagnostics, progress, config, reload, op_queue, cli
  |
  +-- ide  ide-assists  ide-completion  ide-diagnostics  ide-ssr
  |     \      |            |              /               /
  +------  ide-db  --------+-------------+--------------+
  |          |
  +-- hir (Semantics[s2d_cache], SourceAnalyzer[resolver], Adt, GenericParam)
  |     |        <- does NOT depend on ide-db (RA architecture)
  +-- hir-ty (InferenceContext<'db>[db,owner,resolver], method_resolution, Z3)
  |     |
  +-- hir-def (Body+ExpressionStore, ItemTree, DefMap+ItemScope, Resolver, DefWithBodyId)
  |     |
  +-- hir-expand ($include graph, AnalysisScope, InFile<T>, ExpandDatabase)
  |     |
  +-- project-model (.sail_project parser, standalone)
  |
  +-- syntax (rowan CST + typed AST wrappers + ted tree editor)
  |     |
  +-- parser (SyntaxKind, SailLanguage, Event, hand_lexer)
  |     |
  +-- base-db (salsa inputs: FileText, FileId, Durability, SourceRoot, VFS)
  |     |
  +-- vfs / vfs-notify (filesystem abstraction + file watcher)
  |     |
  +-- intern, profile, stdx, paths (foundation)
```

DAG mirrors RA: `syntax → hir-expand → hir-def → hir-ty → hir → ide-db → ide`

### Key design patterns

- **lib + bin split** (sail-lsp): `main.rs` is a thin binary entry; `lib.rs` exposes `main()` and all server logic. Enables `integrated_benchmarks.rs` as `#[cfg(test)]` module.
- **Async dispatch** (sail-lsp): `RequestDispatcher` with `on_sync` (main thread) / `on` (thread pool). RA pattern: handlers get `GlobalStateSnapshot`, main loop never blocks.
- **process_changes**: VFS changes buffered, applied in batch after event handling (RA lazy pattern).
- **op_queue**: Typed operation queue for deferred work (diagnostics, workspace reload).
- **SymbolIndex** (ide-db): O(1) name->location lookup. GotoDefinition/Declaration/Implementation use index instead of O(n) file scan.
- **prime_caches**: Pre-warm salsa queries (parse, item_tree, bodies) after workspace scan for instant first-request response. Parallel phase via rayon.
- **Rowan CST** (parser + syntax): lossless concrete syntax tree, SyntaxKind (~200 variants), event-based Pratt parser.
- **ted.rs** (syntax): tree mutations without re-parsing via rowan splice_children.
- **Body + ExpressionStore** (hir-def): Body embeds ExpressionStore (Arena<Expr> + Arena<Pat>). ExprCollector lowers CST->HIR.
- **Per-item inference** (hir-ty/query.rs): `#[salsa::interned] DefWithBodyId` + `#[salsa::tracked] infer` — edit one function -> only re-infer that function.
- **InferenceContext<'db>** (hir-ty): holds db, owner, resolver, return_ty, diverges, table, is_pure_context. CUSTOM: env (TopLevelEnv), observed_effects.
- **Method resolution** (hir-ty/method_resolution.rs): Candidate, Pick, MethodResolutionContext. CUSTOM: ad-hoc overloading (RA uses trait dispatch).
- **Flow analysis** (hir-ty/flow.rs): control flow for divergence/reachability.
- **SourceAnalyzer** (hir/source_analyzer.rs): bridges syntax<->semantic. Fields: resolver (Arc<DefMap>), BodyOrSig. Methods: resolve_path, resolve_field, resolve_method_call, type_of_expr.
- **Semantics** (hir/semantics.rs): wraps SemanticsImpl with s2d_cache. Entry: analyze() -> SourceAnalyzer. Stub: classify_name_ref.
- **ItemTree typed arenas**: ModItemId enum + per-kind Arena (Function, TypeDef, Register, etc.).
- **DefMap + ItemScope**: DefMap delegates all name lookups to ItemScope (namespace-aware, no parallel HashMap).
- **DefCollector**: Builds DefMap by walking ItemTree + resolving $include dependencies (RA collector pattern).
- **PerNs**: Two-namespace model (Types + Values) for name resolution.
- **AnalysisHost/Analysis**: RA-style snapshot pattern for request isolation.
- **VFS** (vfs crate): VfsPath abstraction, FileSet, content-hash dedup, pending change batching, durability levels.
- **Salsa query chain**: FileText → parse_file → file_item_tree → callable_bodies → DefWithBodyId → infer. Separately: WorkspaceFiles → workspace_context → (reads top_level_env × N files). Include graph: FileText → include_paths → WorkspaceIncludeGraph → AnalysisScope.
- **Workspace context** (hir-ty/query.rs): `workspace_context_query(db, WorkspaceFiles)` is a salsa tracked query (no global static). `WorkspaceFiles` is a salsa input in `base-db`.
- **Effect system** (hir-ty + hir-def): 11 EffectTags, transitive propagation, pure function enforcement during inference.
- **Completion providers** (ide-completion): context analysis -> 11 providers (keyword, expr, dot, pattern, item_list, flyimport, postfix, pragma, record, snippet, type_).
- **Assist handlers** (ide-assists): AssistContext -> handlers::all() -> Assists accumulator (51 handlers, RA pattern).
- **Diagnostic handlers** (ide-diagnostics): 40 handlers with `err_recover` pattern, `DiagnosticsContext` with `sema: Semantics`.
- **Two-stage search** (ide-db/search.rs): index candidate files -> per-file occurrence scan.
- **SSR** (ide-ssr): structural search-replace with `$name` placeholders, full parse/match/resolve/replace pipeline.
- **lsp-types confinement**: only sail-lsp depends on lsp-types; all IDE crates use internal types.
- **Cancellation**: catch_cancelled in dispatch.rs; salsa::Cancelled -> ContentModified error.
- **LRU eviction**: body_with_source_map (512), infer_callable (256), infer_for_body (128).

### Sail-specific features (no RA counterpart)

- **Z3 SMT solver** (hir-ty): numeric constraint solving for dependent types.
- **$include graph** (hir-def): IncludeGraph + AnalysisScope for cross-file scope enforcement.
- **Scattered definitions**: workspace-level completeness checking + scattered clause navigation.
- **Bitfield layout**: visualization in hover table + accessor generation assist.
- **Effect annotations**: per-callable code lens + pure context enforcement during inference.
- **Bitvector witness**: type-level bitvector size reasoning.
- **Outcome effects**: effect tracking through function outcomes.
- **Config truncation**: `config` expression type handling per Sail spec.
- **Pragma completion**: `@name` / `$name` Sail directives.
- **Project visibility**: build_for_project filters symbols by .sail_project boundaries.
- **Bitfield alignment formatting**: align_bitfield_fields for sail-riscv code style.
- **Overload resolution**: Sail's multi-dispatch overloading mechanism.
