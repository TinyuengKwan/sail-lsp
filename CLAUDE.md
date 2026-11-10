# Development guide

Sail-LSP is a Rust workspace with Salsa queries, a Rowan CST and an LSP transport.
Use [CONTRIBUTING.md](CONTRIBUTING.md) for commands, validation tools and conventions;
[architecture](docs/architecture.md) for query flow and semantic boundaries;
[configuration](docs/configuration.md) for supported behavior and limits.

## Implementation rules

- Server handlers call `Analysis` and convert protocol types. Semantic decisions
  belong below the transport; the main loop runs no semantic feature.
- IDE consumers use `hir` and `Semantics`, rather than `hir-*` internals or a
  parallel scrape. `hir` does not depend on `ide-db`; only `sail-lsp` uses LSP types.
- Resolve a name once through semantic definition identity and reuse that identity
  for navigation, references and edits. Keep source-only fallback explicitly local.
- Feature result/config types live with their feature. Pass effective settings
  rather than silently constructing defaults. Completion rendering owns item edits.
- Report diagnostics where facts are established. Keep related spans as locations,
  and compare new semantic diagnostics with the upstream oracle.
- Database writes flow through `Change`/`FileChange`; source roots set durability.
  Release snapshots before writes and cache collection. Keep cache work at idle
  boundaries and advance collection state after input changes.
- Separate semantic content from current positions. Query equality compares values,
  not `Arc` identity. Keep body inference dependent on the selected callable and
  declaration environment rather than unrelated workspace bodies.
- Convert UTF-16 positions through `LineIndex`; keep byte offsets internally.
  Answer client configuration/progress requests and handle cancellation.
- Cancellation remains an error at every `Analysis` entry point. Long operations
  cooperate with cancellation, and obsolete workers cannot publish stale results.
- Check expected types directionally. Recovery and unresolved proofs are not
  established subtype relations. Preserve source locations on obligations.
- Assists produce applicable `SourceChange` values, preserve binding/effect/order
  semantics, and revalidate source versions before applying changes.
- Remove unused mechanisms rather than adding dead-code suppressions.

## Verification and artifacts

Use the checks in [CONTRIBUTING.md](CONTRIBUTING.md#checks-before-committing).
Exercise product entry points with realistic configuration and include contexts.
For bug fixes, verify that the regression fails without the fix. Keep relevant
architecture/source-boundary tests; they complement behavioral tests.

Keep logs, execution reports, comparison snapshots and one-time probes under
ignored `target/` or CI artifacts. Commit reusable tests, policies, workloads and
oracle inputs. Put lasting behavior in its owning guide; avoid dated completion
reports and duplicate command inventories.
