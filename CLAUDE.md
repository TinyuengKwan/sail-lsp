# CLAUDE.md

A multi-crate Rust workspace implementing the **Sail** language server, structured to mirror rust-analyzer's crate layout with salsa-backed incremental queries, rowan lossless CST, and lsp-server async dispatch.

## Development references

Use [CONTRIBUTING.md](CONTRIBUTING.md) for build/test commands, fixtures, corpus
oracles and tool purposes. Use [installation](docs/installation.md) for managed
installation and packaging, and [architecture](docs/architecture.md) for query
flow and Sail semantics. Keep those instructions in their owning document.

## Invariants

Each is held by the named test; read the test before changing the rule.

### Layering

- **The server goes through `Analysis`.** Every feature is one `Analysis`
  method; the handler only converts. `the_server_goes_through_analysis`,
  `handlers_do_not_accumulate_logic` (baseline empty: one feature per handler).
  When `ide` lacks data, give it the data — don't move the decision up.
- **Nothing above `hir` names a `hir-xxx` type.** `ide-db` and up depend on
  `hir` alone; enforced by `Cargo.toml`, so a violation is a compile error.
- **`hir` does not depend on `ide-db`; only `sail-lsp` depends on `lsp-types`.**
- **The vocabulary is Sail's**: `Def`, `TypeDef`, `File`, `TypeVar`,
  `file_def_map`, `DefKind`, `DefId`. `Def` is total over every definition
  form (`every_def_kind_round_trips`).
- **The IDE layer asks the HIR what a name is**, not the
  `ParsedFile` scrape. `the_ide_layer_asks_the_hir` holds the per-file
  baseline; the scrape is still right only for syntax-only facts (comma
  positions) and what the HIR does not model (type variables in a `val`).
- **`Definition` is the currency above `hir`**; `Definition::classify_at`
  (`IdentClass`/`NameClass`/`NameRefClass`) resolves, `FindUsages` and
  `Definition::rename` consume it. Resolve the name once, in the cursor's file.
- **A feature's result type lives with the feature** (`HoverResult` in
  `ide/src/hover`, `CompletionItem` in `ide-completion`), not in `ide-db`.
- **A feature's config is a parameter, not a default.**
- **Completion items are built in `render`**; providers say what to offer.
- **Diagnostics are found once, where the fact is**: name resolution reports
  from the `DefMap` (`AnyDiagnostic::from_def_map`), inference reports
  unreachable code. Only report scattered problems upstream rejects. Check
  the upstream oracle before shipping a lint.
- **`#[allow(dead_code)]` is the compiler's answer, suppressed.** A vendored
  mechanism with no caller is ballast until it has one — wire it or delete it.

### Server loop and protocol

- **The main loop does not run a feature.** Only pure-syntax requests and
  state mutations are sync. `the_main_loop_does_not_run_a_feature`.
- **A cancelled query is an error, not an unwind — at every door.** Every
  `Analysis` method touching the db runs in `Analysis::with_db`.
  `a_feature_answers_through_with_db`.
- **What the server asks the client, it can hear the answer to**
  (`workspace/configuration`, progress tokens, `$/cancelRequest`).
  `the_server_can_hear_the_client_answer`.
- **A client column is a UTF-16 column.** `a_client_column_is_converted_not_assigned`.
- **The server hears about files changing on disk** (dynamic registration,
  `vfs-notify` fallback).
- **The workspace symbol index is a salsa query**, keyed by `FileId`.
- **A change moves the GC nonce**, or LRU eviction never runs again.
  `a_change_moves_the_nonce`.

### Database and incrementality

- **One door into the database**: every write is a `base_db::FileChange`
  through `apply_change`; durability comes from the source root.
  `durability_is_decided_in_one_place`,
  `file_contents_reach_salsa_through_the_file_store`.
- **Never hold an `Analysis` snapshot across a write** — salsa's write waits
  for every live handle and deadlocks.
- **An edit costs the file it touches.** `infer(def)` depends on
  `callable_body(def)`, not the whole file.
  `an_edit_costs_one_file_not_the_workspace`,
  `an_edit_reinfers_only_what_changed`,
  `typing_a_character_does_not_reinfer_the_file`.
- **Query results compare by value, never `Arc::ptr_eq`** — a fresh
  allocation never equals, so nothing backdates. `query_results_compare_by_value`.
- **Salsa backdates the stamp, not the value**: a reused memo keeps its old
  positions. `a_reused_body_still_reports_where_it_is_now`.
- **The ItemTree is an invalidation barrier**: items compare equal
  regardless of offset (exhaustive `PartialEq`).
  `the_same_items_at_a_different_offset_are_the_same_tree`,
  `typing_in_a_body_does_not_redo_name_resolution`.
- **LRU capacity changes require exclusive database access.** Keep collection
  at an idle boundary with no worker tasks or retained snapshots. Body caches
  default to 512 entries; parse, environment and inference capacities are
  configurable. See `cache_control.rs` and `docs/configuration.md#analysis-cache-policy`.

### Type checking

- **A value position with an expectation is checked against it**
  (`infer_expr_hir_with`), except where checking is unsound — notably
  `e : T`.
  `a_value_position_with_an_expectation_is_checked` lists the exceptions.
- **A secondary location is a location**: related spans go in
  `relatedInformation`, not the message.

### Assists

- **An assist test says what the assist does**: `check_assist(before, after)`
  compares text; `$0` marks a cursor, `$0…$0` a selection.
- **An assist has one change, and it can be applied** (`SourceChange` only).
  `every_offered_assist_has_something_to_apply`.

### Measuring

- **Measure through the entry point the product uses, with the inputs it
  has.** `a_probe_builds_the_analysis_the_server_builds`. Before believing a
  coverage number, check what the probe gives the feature, not just which
  function it calls.
- **Group a miss by its ancestor chain, not its node** (`SAIL_DBG_HOVER=1`).
- **Check that a new test can fail** — put the defect back and run it.
- **Keep run-specific evidence in `target/` or CI artifacts.** Commit regression
  tests, workloads and reproduction commands; document lasting limits in the
  relevant guide rather than adding another dated execution report.

## Documentation and artifacts

Keep execution reports, logs, temporary probes and comparison snapshots under
ignored `target/` or CI artifacts. Commit reusable tests, workloads, oracle inputs
and reproduction commands. Put lasting behavior and limitations in the relevant
user/developer guide; do not add dated completion reports or duplicate command
inventories here.
