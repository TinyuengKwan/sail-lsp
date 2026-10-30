# Editor experience work

Historical snapshot: results below belong to the recorded 2026-10-25 tree and
corpora. Current measurements and the P2/P3 follow-up are in
[2026-10-29-editor-p2-p3.json](2026-10-29-editor-p2-p3.json); do not combine
these counts or treat old completion states as current coverage.

Scope: all items in the requested todo list except P1-1 (remote build identity)
and P1-4 (Zed-specific integration). Completion requires evidence below; an
existing implementation alone is not an acceptance result.

| Item | Required evidence | State |
| --- | --- | --- |
| P1-2 | Real stdio RISC-V startup/edit/completion/hover/navigation measurements, latency distributions and cancellation outcomes | Verified: two isolated final stdio runs, 100 edits each, p50/p95/p99/max and 40 successful cancellations; no server error logs |
| P1-3 | Rapid edits/undo, stale publications, cross-file signature changes and diagnostic recovery | Targeted scenarios and server checkpoint passed; verified in final default/no-SMT workspace gates |
| P1-5 | Incomplete-code completion, ranking and applied edit/snippet correctness | Configuration/range/context/snippet application tested; local-at-body-end bug fixed; ranking protocol test passed; verified in final default/no-SMT workspace gates |
| P2-1 | Hover signature clarity and nested-call active parameter tests | Nested calls, token ranges, member limits and scoped/bounded overload display tested; verified in final default/no-SMT workspace gates |
| P2-2 | Cross-file declaration/mapping/scattered navigation, references and safe rename | Function/mapping/scattered definition/declaration/implementation targets and token ranges checked; rename applied/rechecked, including shadowed locals and Unicode target ranges; verified in final default/no-SMT workspace gates |
| P2-3 | Diagnostic-to-fix coverage audit and applied fix reanalysis | Coverage audit recorded separately; literal/pattern fields, guarded/nested match witnesses, unused locals, syntax and stale-diagnostic safety verified; verified in final default/no-SMT workspace gates |
| P2-4 | Hint density/truncation, lens value, semantic token stability and defaults | Parameter identity/length/brace thresholds tested; 20-edit exact token-delta test, lens categories and master configuration switches passed; verified in final default/no-SMT workspace gates |
| P2-5 | Repeated open/close/edit/reload resource measurements and recovery | Verified: final 20/10 reload cycles plus idle tails, 56 settled threads, 264/263 MiB heap; idle RSS 496/447 MiB; panic/cancellation recovery tests passed |
| P3-1 | Remaining budget-exhausted goals examined and optimized without raising production budgets | Verified: two isolated reports agree with zero budget exhaustion/refutations; strict and both workspace modes passed |
| P3-2 | Upstream virtual-file semantics, project applicability and generated-source integration | Implemented: typed __virtual values, registered physical sources, corelib provider, watched updates and overlays; targeted model/server tests passed; verified in final default/no-SMT workspace gates |
| Regression gate | Strict upstream pass 276/276 and fail 217/217; appropriate workspace checks | Verified: strict 276/276 + 217/217; default 2388 and no-SMT 2373 passed, each 17 ignored; all-targets/fmt passed |
| Previous ablation | Review and commit retained TextDocument simplification and report | Complete: a66b7c3; 32 TextDocument tests passed again |

All scoped items are now verified. P1-1 and P1-4 remain excluded as requested.
The final source and binary hashes, per-stage outcomes and measured results are
retained in `2026-10-25-editor-probe.json`; constraint repeats are retained in
`2026-10-25-editor-constraints.json`. Earlier entries below are a historical work
log, including failed experiments and superseded pending states.

## Historical implementation and validation log

- `diagnostics.rs`: three new tests failed on the original code (stale batch
  merged into a newer result, unchanged results failed to advance generation,
  code/related-information changes suppressed). The collection now rejects
  superseded waves, advances identical-result generations and compares full LSP
  diagnostics. A fourth test covers arrival before the replacement completes.
- Server protocol test covers signature changes in one open file, dependent
  errors in another, 30 incomplete/invalid edits followed by undo, and agreement
  between push and pull. The 294-test server run passed before the later
  completion/hover changes; final stage run is recorded separately below.
- Further diagnostic review found the nominal semantic wave includes syntax
  diagnostics already. Publication now selects the full result for its revision,
  rather than concatenating it with syntax or mixing new syntax with old type
  errors. Closing the last buffer also invalidates pending waves. Retained old
  native results and compiler-on-disk results are not labeled with the current
  buffer's version. The syntax push/pull test normalizes ordering before comparing.
- Completion settings previously had no path from the request to the providers.
  Configuration now reaches candidates, controls limits/templates/postfix/call
  parentheses and respects snippet settings at serialization. New protocol test
  toggles configuration and verifies existing `(x)` is not duplicated. Two IDE
  tests cover plain keywords with templates disabled and the postfix toggle.
- Primary completion edits now have explicit physical replacement ranges,
  including the receiver for postfix transformations and the remainder of an
  identifier when completing in its middle. Resolve preserves the original
  primary edit. Postfix templates no longer emit doubled literal/snippet braces.
  The protocol test applies `.return`, checks exact source, resolves the item,
  and checks the resulting buffer through the normal diagnostic pipeline.
- Prefix filtering now precedes per-declaration/member visibility resolution.
  It preserves the same visibility predicate for every retained candidate.
  The pre-configuration-change IDE/completion run passed 572 + 24 tests.
- Hover inside `creg2reg_idx` returned columns 11..23 instead of the token's
  9..21. The protocol regression failed before using the lexer's token span.
- `scripts/editor_probe.py` speaks framed stdio LSP to the release binary, stores
  responses/notifications and process RSS, and changes only unsaved buffers.
  See `2026-10-25-editor-probe.json` for exploratory measurements and caveats.
  Initial probe completion at a declaration returned null; that run is excluded
  from the retained comparison. The retained probe completes `zero_ext` in an
  expression and requires nonempty post-load feature responses.
- Generated-file audit found actual upstream syntax is `__virtual`, used by
  `sail/lib/core/corelib.sail_project`; upstream registers its contents through
  `Sail_file.add_virtual_file`. Added a distinct virtual value with upstream path
  composition/equality semantics, a registered-source map and the standard
  corelib provider. `workspace.generatedFiles` supplies producer output paths;
  locations remain physical generated-Sail locations. Tests cover unknown names,
  inactive conditional branches, effective stdlib selection, navigation, unsaved
  overlays, producer disk changes and switching the provider via configuration.
  Configuration documentation and VS Code's setting schema are updated.

Intermediate validation (not the final goal audit):

- Workspace checkpoint through initial generated-file support: 2,350 passed,
  0 failed, 17 ignored, 55 suites (`target/editor-workspace-tests-final.log`).
  Later primary completion edits and diagnostic publication refinements were
  added afterwards and need a final workspace rerun.
- Latest broader server run: 299 passed, 1 ignored
  (`target/editor-server-stage2.log`); the subsequently added syntax push/pull
  test passed separately (`target/editor-syntax-push-test-sorted.log`).
- Current source checkpoint: 301 server tests passed, 1 ignored
  (`target/editor-server-checkpoint.log`); release binary rebuilt successfully.
  All-targets check passed (`target/editor-all-targets-latest.log`).
- Strict corpus: 276/276 pass, 217/217 fail, no false positives or false negatives
  (`target/editor-strict-conformance.log`). This release build includes project
  virtual-file support; later edits only affect IDE/protocol paths.
- Isolated lifecycle checkpoint (`target/editor-probe-lifecycle.json`): 60 edits,
  12 reference requests canceled with `-32800` (all answered within 2 ms), and
  10 reopen/reload/close cycles completed. Editing completion p95 23.797 ms,
  hover p95 256.504 ms, definition p95 2.680 ms. Reopened requests all succeeded.
  RSS was ~553 MB after edits, ~725 MB after ten reload cycles, ~764 MB after
  ten idle seconds. Priming was still a possible contributor; this is not
  evidence of resource recovery or proof of a leak. Extend the probe to wait
  for actual cache-prime completion and sample longer settled cycles next.

Remaining work includes protocol cancellation/lifecycle stress and isolated
measurements, completion range/snippet application and context audit, diagnostic
noise and mixed-generation publication review, navigation/rename and quick-fix
round trips, hint/lens defaults, long-session resource recovery, remaining solver
goals, and the final strict corpus/workspace gates.

Latest continuation evidence:

- Record-fill actions now use inference metadata, including generic field types,
  without adding anonymous-record diagnostics. Cross-file named/anonymous record
  apply-and-recheck test passed (`target/editor-record-fill-protocol.log`);
  all 128 assist tests passed (`target/editor-all-assists.log`).
- Real protocol testing exposed a parameter-hint bug missed by the ad-hoc fixture:
  signature display labels (`arg1: int`) were treated as binding names. Parameter
  names now come from implementations with the selected canonical identity.
  Cross-file replacement, declaration-only calls, real parameters named `arg1`,
  length limits and tooltip resolution pass two protocol tests
  (`target/editor-inlay-protocol-complete.log`). All 13 IDE inlay tests pass
  (`target/editor-inlay-final.log`).
- Hover's extra-signature display now uses call-site visibility instead of
  collecting all same-named declarations globally. A four-file replacement test
  checks both the old and new call sites (`target/editor-hover-scope.log`).
  Long/overloaded display audit remains pending.
- The full server suite passed: 305 passed, 0 failed, 1 ignored
  (`target/editor-server-stage3.log`).
  The settled lifecycle probe was confirmed live by process inspection; its
  final resource report has not yet been reviewed. Do not infer a leak or
  recovery from intermediate RSS.

- Hover member limits now reach rendering and server configuration. Defaults
  show ten struct/union fields or enum variants; null shows all. Both headers
  and fenced signatures are bounded, with omission counts. Nested tuple/vector
  types, comments, and type-parameter set constraints preserve member boundaries.
  All 19 hover IDE tests pass (`target/editor-hover-display.log`); protocol
  configuration/null/zero-limit test passes (`target/editor-hover-limits-protocol.log`).
- Settled lifecycle probe completed successfully: 100 edits, 20 reloads, 20
  cancellations, normal shutdown. RSS after edits: 552652 KiB; after reload 20:
  818444 KiB, unchanged after 30 idle seconds. Threads remained at 56 across
  settled reload samples. Recovery remains unproven. The retained JSON now
  includes this run and its build/concurrency limitations. An allocator-arena
  control run is in progress (`target/editor-probe-settled-arena2.json`);
  no production allocator change has been made.
- Current-source analysis_stats/constraint collection is running in
  `target/editor-current-riscv.log`, report `target/editor-current-constraints.json`.

- Semantic-token protocol test now applies every delta to the cached integer
  stream and compares it exactly with a full response after each of 20 edits:
  insert/delete/incomplete syntax/Unicode/undo. Passed in
  `target/editor-semantic-delta-stress.log`.
- Cross-file rename apply/recheck now covers ordinary functions, mappings and
  scattered functions (including head/clause/end). All three scenarios passed
  in `target/editor-rename-mapping-scattered.log`. Identity safety and navigation
  coverage still need final audit.
- Fresh constraint report (`target/editor-current-constraints.json`): 3803
  obligations, 3763 proved, 0 refuted, 40 unknown; budget_exhausted=3,
  erased_numeric=12, flexible_witness=21, not_established=4. No diagnostic
  errors (62 warnings, 6 hints).
- Added exact natural-power ordering/positivity rules. The full project report
  (`target/editor-power-constraints.json`) proves one more obligation, reducing
  budget_exhausted to 2, with unchanged diagnostics. Missing-premise and signed
  small-domain tests passed. All 72 semantic audit tests passed serially;
  concurrent run had three direct SMT timeout-sensitive tests fail, so do not
  report the concurrent run as green (`target/editor-solver-semantic-audit.log`,
  `target/editor-solver-semantic-audit-serial.log`). Production limits unchanged.
- Positive-integer factor upper bounds from known positive products added with
  counterexample and enumerated-domain checks (`target/editor-solver-product.log`).
  Full-project validation running in `target/editor-product-riscv.log`, report
  `target/editor-product-constraints.json`. Solver work remains incomplete.

- Four completion contexts now have protocol apply/resolve/recheck coverage:
  expression/local, type name, record field, enum pattern. Completing in the
  middle replaces the full identifier suffix, preserves resolve edits, and
  leaves no diagnostic errors (`target/editor-completion-contexts.log`).
- Product rule removed the `vmem_read_addr` byte-size budget-exhausted site.
  The project run still has two such sites because `execute_vsetvl_type`
  timed out again; the previous count improvement is not yet stable across
  loaded-machine runs (`target/editor-product-constraints.json`).
- Added sufficient linear-premise proof before SMT power expansion. It proves
  natural exponent order only, and falls back to the original goal when the
  premise is not established. The vector-length fixture and missing-premise
  control passed (`target/editor-vector-power-premises.log`). Current semantic
  audit and strict conformance runs are in `target/editor-solver-semantic-stage2.log`
  and `target/editor-strict-stage2.log`; no final gate claim yet.
- analyzerStatus now includes the existing profile allocator heap estimate;
  the probe's optional `--heap-status` collects it at settled checkpoints.
  This helps distinguish active allocations from RSS retention. The estimate
  is platform/allocator dependent and is not itself a leak verdict.

- Strict upstream checkpoint after linear power-premise support passed:
  pass 276/276, fail 217/217 (`target/editor-strict-stage2.log`). Quotient-premise
  work was added afterwards and still needs the final gate rerun.
- Added sufficient product/quotient premises for vector element-count bounds,
  covering both `/` numeric nodes and built-in `div(...)` calls. Focused vector
  case passed (`target/editor-vector-quotient.log`); all 75 semantic audit tests
  passed serially (`target/editor-solver-semantic-stage3.log`). Separate signed
  domains, including zero/negative divisors, verified each implication by exact
  evaluation (`target/editor-quotient-soundness.log`). Full RISC-V count pending.
- Added optional `jemalloc` feature using the local rust-analyzer driver's
  allocator pattern and the workspace profile crate's existing jemalloc support.
  System allocator remains default. Matched system binary saved as
  `target/editor-allocator-system`; jemalloc build in progress in
  `target/editor-allocator-jemalloc-build.log`. Do not claim a production memory
  fix until allocator/resource measurements justify one.
- New unused-binding/var-to-let protocol apply-and-recheck test is running in
  `target/editor-binding-fixes.log`. The prior allocator arena control probe
  remains live (confirmed by process inspection); do not restart it just because
  it is slower than the default run.

- Arena-count diagnostic control finished normally: after 20 settled reloads
  RSS was 503752 KiB and remained there after 30 seconds idle (default run:
  818444 KiB). Most arena-control samples were 503468..503764 KiB; one was
  512024 KiB. This is evidence of allocator retention/fragmentation contribution,
  not proof that every cache is bounded. Run retained in editor-probe.json.
- Jemalloc release build succeeded; 100-edit/20-reload probe with heap estimates
  is live (`target/editor-probe-jemalloc.json`, progress in `.summary.log`).
- Quick-fix audit reproduced a real wiring defect: unused_variable_fix had
  direct unit tests but was not called from quick_fix_for_diagnostic. Connected
  it; real codeAction/resolve/apply/recheck now removes the unused-local warning
  without introducing errors (`target/editor-binding-fixes.log`).
  The var-to-let diagnostic has no producer, and unused parameters are not
  currently warned about; these are not claimed as tested client fixes, and no
  new warnings were added just to make a fix scenario run.
- Current RISC-V validation of quotient premises is running in
  `target/editor-quotient-riscv.log` / `target/editor-quotient-constraints.json`.

- Latest quotient-premise project run completed: 3803 obligations, 3766 proved,
  0 refuted, 37 unknown; **zero budget_exhausted**. Remaining reasons are
  erased_numeric=12, flexible_witness=21, not_established=4. Diagnostics unchanged
  (62 warnings, 6 hints, no errors). Before/after budget sites retained in
  `2026-10-25-editor-constraints.json`. Isolated repeats and final regression
  gates are still required before claiming stable completion of P3-1.

- Full workspace checkpoint: 2368 passed, 0 failed, 17 ignored across 55 suites
  (`target/editor-workspace-stage4.log`). Later hover overload limits, stale
  code-action guards, completion body-end correction and Z3 recovery changes
  require a final rerun. Server stage5: 309 passed, 1 ignored before the last
  completion/Z3 changes (`target/editor-server-stage5.log`).
- Bounded overload hover uses call-site semantic membership/signatures, with a
  configurable count and omitted-count display. Cross-file replacement test
  passed (`target/editor-hover-overloads.log`).
- Cross-file navigation now checks definition/implementation/declaration URI
  and exact identifier token ranges for functions, mappings and scattered
  functions, alongside rename apply/recheck (`target/editor-navigation-roundtrip.log`).
- Ordinary lazy code actions now carry a source fingerprint and reject resolve
  after edits with ContentModified. Apply/recheck/stale-resolve test passed
  (`target/editor-binding-fixes-stale.log`).
- Completion ranking test exposed lost locals at an expression body's exclusive
  end. Scope lookup now anchors at the identifier prefix start. Ranking and
  snippet default application tests passed (`target/editor-completion-ranking.log`,
  `target/editor-function-snippet.log`). Missing closing-brace/paren actions
  apply/recheck successfully (`target/editor-syntax-fixes.log`). Native block
  parsing accepts omitted semicolons, so that fixture is not claimed as a
  diagnostic quick-fix scenario.
- Jemalloc without background purge did NOT pass lifecycle validation: after
  reload-close-16, Z3_simplify returned null after cancellation; the worker
  panicked and never closed cache-priming progress. Probe timed out and retained
  failure (`target/editor-probe-jemalloc.json` plus stderr log). Do not report
  this run as stable or complete.
- Removed interruptible simplification from numeric power translation and
  concrete power facts; use Rust BigInt values instead. Native-cancellation test
  now checks translation of subsequent concrete/symbolic powers and passed
  (`target/editor-z3-after-cancel.log`). Priming catches panic and sends a failed
  end event, clears the queue, logs failure, and allows later reload; targeted
  recovery test/build are in `target/editor-prime-failure-recovery.log` and
  `target/editor-background-recovery-build.log`. Probe now detects explicit
  priming failure immediately. Jemalloc candidate enables background purge;
  new lifecycle validation is still required.

- Recovery test passed. Background-purge/recovery candidate rebuilt and copied
  to `target/editor-allocator-background`; fresh 100-edit/20-reload probe is live
  (`target/editor-probe-background-recovery.json`, `.summary.log`). Previous
  failed probe is terminal and retained in the durable probe report. No
  production allocator default has been changed yet.

- Unicode cross-file references now convert each location using its target
  file's line index (the old code used the requesting file). The protocol
  regression first reproduced the wrong token slice, then passed after the fix
  (`target/editor-navigation-unicode.log`). CodeLens category switches now
  reach the provider; independent categories and master display switches passed
  (`target/editor-display-config.log`).
- Further quick-fix audit confirmed diagnostic-handler fixes are intentionally
  not transported to code actions. They cannot simply be enabled: type-mismatch
  fixes still substitute literal `expr`, propose incorrect int/nat conversions,
  and legacy SourceChange constructors default to FileId 0. Audit remains open;
  fix providers must use semantic identity, actual source and explicit file IDs.
- The incomplete-match quick fix scanned raw characters after the diagnostic
  range and inserted a unit-valued arm. It now selects the smallest containing
  MATCH_EXPR and its direct closing token, handles missing/trailing separators,
  and labels the new `undefined` arm explicitly as a placeholder. Targeted tests
  cover whole-expression diagnostic ranges, nested expressions and comment braces.
  Protocol apply/reanalysis remains required. The separate add_missing_match_arms
  assist also needs repair: it currently selects variants from arbitrary local
  enums instead of the inferred scrutinee type (and uses todo calls).
- Missing-match assist now consumes Semantics::missing_match_patterns, which
  returns inference's exhaustiveness witnesses for the exact expression. It no
  longer selects arbitrary local enum members or treats a partially covered
  constructor as fully covered. Bodies use explicitly unfinished `undefined`
  snippet defaults. Six targeted tests passed, including competing enums and
  Some(true)/Some(false) payload coverage (`target/editor-semantic-match-arms.log`).
  Cross-file protocol application and guarded/nested patterns remain to validate.
- Recovery/background-purge lifecycle probe completed normally: 100 edits,
  20 reloads, 20 cancellations, exit 0. Last settled RSS 640672 KiB; after
  30 seconds idle 636856 KiB; heap estimate 256 MiB (initial 254 MiB), threads 60.
  No priming panic/stall. Retained report in editor-probe.json. Optional allocator
  result is not a default-build memory fix; isolated repeats still required.
- Cross-file match codeAction/resolve/apply/recheck passed for both generated
  exhaustiveness witnesses and the explicit wildcard placeholder, including a
  trailing comma/comment containing `}`. No remaining incomplete-match diagnostic
  or error after applying (`target/editor-match-protocol.log`).
- Workspace stage6 parallel run failed 8 hir-ty tests (423 passed), including
  direct SMT Unknown results and two explicit BudgetExhausted obligations.
  Preserve `target/editor-workspace-stage6.log`; do not count it as a green gate.
  Serial whole-workspace run started in `target/editor-workspace-stage6-serial.log`
  to separate concurrency/time-budget sensitivity from semantic regressions.
- Source-action audit found `remove_unused_imports` deleted includes whenever
  the current file had no calls, even when types depended on the included file.
  Removed this unsound provider; cross-file type-only dependency protocol test
  passed (`target/editor-include-action-safety.log`). Sail includes may also
  affect preprocessing, so absence of symbol references alone cannot justify
  removal. No new unused-include diagnostic is introduced.
- The related organize action sorted and deduplicated includes, which changes
  Sail's source-order/preprocessor semantics. It now preserves order/repetition
  and explicitly presents itself as "Normalize include formatting". Tests check
  whitespace normalization while retaining repeated includes in original order.
  Full assist suite result: `target/editor-assists-include-safety.log`.
- Added `2026-10-25-quick-fix-coverage.md`: all 16 diagnostic handlers carrying
  fixes audited against actual client dispatch, including unsupported/dormant
  metadata and remaining pattern validation. Do not count handler tests as
  protocol coverage.
- Serial stage6 passed hir-ty but failed one IDE assertion expecting the old
  overload heading (`members` instead of bounded `2 overloads`). Updated that
  assertion; targeted test passed (`target/editor-hover-final-label.log`). This
  does not turn the interrupted workspace run into a successful full gate.
- Cache priming recreated rayon workers on every reload. Added a bounded cache
  of the current configured pool, retaining old pools only for in-flight users.
  Reuse/reconfiguration/old-user lifetime and active-file priming tests passed
  (`target/editor-prime-pool-reuse.log`). Default-allocator lifecycle measurement
  is required before claiming memory improvement. Release rebuild is running in
  `target/editor-final-release-build.log`.
- Missing record patterns now use inference metadata too. New fields are wildcard
  patterns, not default expressions that would narrow coverage. Unit test and
  cross-file generic pattern codeAction/resolve/apply/recheck passed, including
  existing trailing commas and absence of new incomplete-match warnings
  (`target/editor-pattern-fields.log`, `target/editor-pattern-fields-protocol.log`).
- Strict current-solver release checkpoint passed: 276/276 clean, 217/217 rejected,
  zero false positives/negatives (`target/editor-strict-stage3.log`). Built after
  Z3 recovery and pool reuse, before subsequent IDE-only pattern-field changes.
- Default allocator + reused inference pool lifecycle probe started using that
  release binary, 100 edits/20 reloads/20 cancellations/30 idle seconds, report
  `target/editor-probe-reused-pool.json`. No build/test work should overlap this
  measurement. Probe now reports p99 as well as p50/p95/max. Result pending.
- Added guarded-boolean and nested-union witness tests, plus local-rename protocol
  coverage for shadowed globals/comments/strings. Execution deferred until the
  isolated default-allocator probe finishes (session 76071; live process confirmed).
- Code-action audit found the existing resolve fingerprint does not reject stale
  diagnostics already supplied with a *new* code-action request. Added current
  snapshot diagnostic validation before diagnostic-specific edits, and a protocol
  regression where a previously unused binding has become used. Test execution
  pending; do not count as verified yet.
- Isolated default-allocator/persistent-pool probe finished normally: exit 0,
  100 edits, 20 reloads and cancellations. Idle RSS 861036 KiB, heap estimate
  396 MiB, 68 threads. Editing p95/p99: hover 55.763/80.126 ms, completion
  17.542/17.976 ms, definition 2.879/2.980 ms. Retained in editor-probe.json.
  Persistent pool did not improve retained memory; reverted that experiment
  completely, retaining measurements. Do not claim pool reuse as a delivered fix.
- Post-probe full workspace serial validation started in
  `target/editor-workspace-stage7.log`, including new guard/nested witness,
  stale-client-diagnostic, shadow-safe rename and record-pattern tests.
- New safety checks passed: stale client diagnostic cannot rename a now-used
  binding (`target/editor-stale-client-diagnostic.log`); local rename updates
  exactly its binding/use while preserving global, comment and string spellings
  (`target/editor-local-rename-identity.log`). All 8 match-witness tests passed,
  including guarded bool and nested constructor payloads
  (`target/editor-match-witness-final.log`). Full stage7 remains in progress.
- Stage7 default-feature unit/integration suites completed successfully, but
  rustdoc then failed to load ide_db/ide_completion artifacts while another
  feature configuration and all-target checking used the same target directory.
  Treat this as an incomplete gate; sequential doc rerun is in
  `target/editor-doc-stage7.log`. Avoid overlapping feature builds for final gates.
- No-Z3 workspace gate found four unsupported_audit cases that demand solved
  arithmetic/flow obligations but receive NotEstablished without SMT. Retained
  `target/editor-no-smt-stage7.log`; do not hide this as a passing configuration.
- Added best-effort Linux/glibc free-page reclamation after cache-priming workers
  and their snapshots are dropped, before reporting completion. Runs on the
  background worker, never per interaction or on the event loop. Other platforms
  keep normal allocator behavior. Release build passed
  (`target/editor-reclaim-build.log`); isolated lifecycle evaluation pending.
- All-target check passed (`target/editor-all-targets-stage7.log`). Rustfmt also
  identified 12 preexisting format-only files; normalized them alongside changed
  files. Full formatting check passed (`target/editor-format-final.log`), with
  original file list retained in `target/editor-format-preexisting.json`.
- Sequential workspace doc rerun passed (exit 0, `target/editor-doc-stage7.log`),
  resolving the artifact-load failure. Default stage7 unit/integration suites
  had 2354 passed, 6 ignored across 38 successful suite summaries before docs.
- Reclamation candidate isolated stdio probe is now running in
  `target/editor-probe-reclaim.json` / `.summary.log` (100 edits, 20 reloads,
  20 cancellations, 30-second idle tail). Build/test activity paused for sampling.
- No-SMT failure analysis: finite membership arithmetic lacked exhaustive case
  analysis, and negated comparison facts were omitted from the difference graph.
  Added bounded positive-membership splitting (64 states shared across the whole
  search, never sampling a partial domain) and exact complementary comparison
  edges. Tests cover arithmetic/division, set-bound loop indices, missing premises,
  disjunction/negation, unknown members, domain cap, signed-domain soundness and
  all four ordered comparison complements. Test execution deferred until isolated
  reclamation probe completes; this is not yet a no-SMT gate success.
- Reclamation lifecycle probe passed: 100 edits, 20 reloads/cancellations, exit 0;
  RSS after reload 20 = 464048 KiB, after 30 seconds idle = 464120 KiB, heap
  estimate 266 MiB, 56 threads. The earlier untrimmed system run ended at
  818444 KiB. Editing p95/p99: completion 17.657/19.191 ms, hover 56.384/57.033 ms,
  definition 2.776/3.039 ms. Full report retained in editor-probe.json.
  Keep reclamation; remove the experimental server jemalloc feature/dependency
  and alternate global allocator. Default allocator stays unchanged. Linux/glibc
  is the measured platform; other platforms retain their own reclamation policy.
- Current no-SMT solver suite running in `target/editor-no-smt-solver-stage8.log`
  with exact finite-domain and complementary-edge changes; no test weakening.
- No-SMT hir-ty suite now passed: 421 tests, 0 failed, including all four formerly
  failing unsupported-audit cases and the new finite-domain/negation soundness
  checks (`target/editor-no-smt-solver-stage8.log`). Full no-SMT workspace gate
  started sequentially in `target/editor-no-smt-stage8.log`; no overlapping
  builds with different feature sets. Default/strict solver reruns still pending.
- Full no-SMT workspace gate passed: 2372 passed, 17 ignored, 55 suite summaries,
  exit 0 (`target/editor-no-smt-stage8.log`). This includes docs and all new
  semantic/editor changes, without weakening assertions or increasing budgets.
- Sequential current-source acceptance runner started (`target/run_editor_acceptance.py`,
  log `target/editor-final-acceptance.log`, per-stage status
  `target/editor-final-acceptance-state.json`). It runs default workspace tests,
  all targets, formatting, release build, strict corpus, two isolated constraint
  reports, then two isolated stdio probes (20 and 10 reloads). Stops on nonzero
  command exits, strict count mismatch, refuted obligations or budget exhaustion.
  No concurrent builds/tests should interfere with its feature sets or sampling.

## Completion evidence index

This index ties requirements to actual paths exercised, not just implementation
presence. Final runner results have been reviewed; all eleven stages passed.

| Requirement | Authoritative checks |
| --- | --- |
| P1-2 interaction/cancellation | `scripts/editor_probe.py`: framed stdio initialize, nonempty post-load hover/completion/definition, 100 unsaved edits, 20 reference cancellations, p50/p95/p99/max, response errors, clean exit; final two reports verified |
| P1-3 diagnostic consistency | `rapid_signature_edits_and_undo_recover_dependent_diagnostics`, `syntax_errors_are_not_duplicated_by_the_full_diagnostic_wave`; collection tests for superseded waves, metadata changes, identical-result generation advancement, closing/undo recovery |
| P1-5 completion | `completion_settings_reach_candidates_and_preserve_existing_calls`, `completion_replaces_identifier_suffix_in_expression_type_field_and_pattern`, `completion_ranks_a_local_binding_before_a_global_with_the_same_prefix`, function snippet and postfix apply/resolve/recheck tests |
| P2-1 hover/signature | scoped cross-file overload/member-limit protocol tests, exact token ranges, `nested_calls_select_the_innermost_argument_and_return_to_outer_call`, initializer signature test |
| P2-2 navigation/references/rename | `cross_file_rename_updates_signature_implementation_and_calls` covers functions/mappings/scattered declarations and exact target token ranges including Unicode; `local_rename_preserves_shadowed_global_and_non_code_occurrences` applies edits and rechecks |
| P2-3 actionable diagnostics | `2026-10-25-quick-fix-coverage.md`; field literal/pattern, missing match witnesses, unused local, closing delimiter, auto-include round trips; stale request and stale resolve guards; unsafe include cleanup removed |
| P2-4 display/configuration | `display_configuration_controls_lens_categories_hints_and_tokens`, parameter identity/length tests, brace threshold tests, `a_semantic_token_delta_describes_the_buffer_now` compares 20 applied deltas with full streams |
| P2-5 lifecycle/recovery | 20- and 10-reload final probes; settled heap/RSS/thread samples and idle tail; priming panic recovery and native solver cancellation tests; no allocator switch or persistent inference pool retained |
| P3-1 solver budgets | two final constraint reports must have 0 refuted and 0 budget_exhausted; strict 276/276 + 217/217; exact signed-domain, missing-premise and finite-domain budget soundness tests; production budgets unchanged |
| P3-2 generated/virtual projects | `virtual_files_use_registered_sources_and_preserve_value_kind`, `virtual_sources_support_navigation_unsaved_edits_and_provider_changes`; manifest selection, producer disk updates, overlay and mapping-change protocol coverage |
| Regression/build | both workspace modes, all-targets, fmt and strict counts verified |

Not included: P1-1 remote build identity and P1-4 Zed-specific integration, as
requested. Diagnostic fixes that cannot be generated soundly remain explicitly
listed as unsupported; synthetic handler metadata is not counted as a delivered
editor action. Memory reclamation is measured on Linux/glibc; other platforms
keep their native allocator policy.

- Final default workspace gate passed: 2387 passed, 17 ignored, 55 suites,
  exit 0, including docs (`target/editor-final-default-workspace.log`). Current
  all-targets and rustfmt checks and release build also passed. Code remained
  unchanged throughout; only acceptance documentation was updated. Strict and
  isolated constraint/performance repeat stages are still running in the same
  acceptance runner (session 82524), not restarted.
- Strict final checkpoint passed 276/276 + 217/217. Constraint repeat then
  correctly stopped acceptance: first run 3765 proved/37 unknown, second
  3764 proved/38 unknown, both 0 refuted out of 3802; second had one budget
  exhaustion in vext_arith_insts.sail:1888, width / 2^shift membership under
  width in {8,16,32,64}, shift in [1,3], quotient >= 8. No performance stages ran.
  Preserve `target/editor-final-constraints-{1,2}.json` and runner failure state.
- Extended bounded domain proof to use the existing exact interval-domain
  extractor, prioritizing goal variables and smaller domains. Shared 64-state
  limit unchanged. Focused vector division proof and missing-premise/signed/
  disjunctive controls passed without SMT (`target/editor-vector-domain-proof.log`).
- Round9 sequential acceptance started, with logs/state prefixed editor-round9;
  runs no-SMT solver, release/strict/two real-project reports first, then default
  and no-SMT workspace gates and isolated performance probes. Old failure reports
  retained. New solver changes mean prior full workspace counts are checkpoints,
  not final evidence for the updated implementation.
- Round9 strict corpus passed again: 276/276 clean and 217/217 rejected.
  Both isolated current-source constraint runs agree: 3765 proved, 0 refuted,
  37 unknown out of 3802; budget_exhausted=0 in both. Remaining unknown categories
  unchanged: erased_numeric=12, flexible_witness=21, not_established=4. Diagnostics
  remain 62 warnings, 6 hints, no errors. Durable constraint report now retains
  prior failed repeat and both successful repeats with report hashes.
- Round9 default workspace gate is running. Source hashes for code/config were
  recorded in `target/editor-round9-source-hashes.json` for final verification.

- Round9 final default workspace: 2388 passed, 17 ignored, 55 suites, exit 0.
  Final no-SMT workspace: 2373 passed, 17 ignored, 55 suites, exit 0. Both include
  docs. All-targets and formatting passed. Recorded source hashes match exactly
  after these gates. First final performance/lifecycle probe started; second
  repeat will follow in the same runner. No code changes or concurrent tests.

## Final acceptance

- Scope complete: P1-2, P1-3, P1-5, P2-1 through P2-5, P3-1 and P3-2.
  P1-1/P1-4 were not attempted. Previous ablation commit remains a66b7c3.
- All eleven sequential runner stages passed, exit 0. Default workspace:
  2388 passed, 17 ignored; no-SMT workspace: 2373 passed, 17 ignored. Each includes
  55 suite summaries and docs. All targets compile; formatting/diff checks pass.
- Strict upstream corpus: pass 276/276 clean, fail 217/217 rejected, zero false
  positives/negatives. Two constraint reports identical: 3765 proved, 0 refuted,
  37 unknown; no exhausted budgets. Solver budgets were not raised.
- Two independent stdio processes, 100 edits each; 20 and 10 settled reloads,
  30-second idle tails; 40 client cancellations all answered with -32800;
  no non-cancellation request errors and no ERROR-level server logs.
- Editing p95/p99 ms: completion 17.024/17.337 and 17.236/17.689;
  hover 55.916/63.850 and 56.531/59.481; definition 2.597/2.678 and 2.730/2.856.
  Ready notifications at 1123.720 and 1122.593 ms (background priming continues).
- Idle RSS 507692 and 457916 KiB (496/447 MiB), settled threads 56,
  allocator heap estimates 264/263 MiB. Earlier untrimmed system run ended at
  818444 KiB. Measurements are Linux/glibc, not a portability or RA parity claim.
- Source hashes stayed unchanged throughout the final gates. Both probes used
  binary SHA256 `a30efb812712c96a7fa0262d922fd28c4cd95dcc4c8c92d87c34556b619426bd`,
  matching the final release binary. Experimental jemalloc and persistent-pool
  changes were removed; retained failed reports explain those decisions.
- Diagnostic-to-fix coverage and intentional unsupported cases are documented in
  `2026-10-25-quick-fix-coverage.md`. Existing dormant metadata is not counted as
  a working fix, and the 37 unknown obligations are not claimed as proved.
