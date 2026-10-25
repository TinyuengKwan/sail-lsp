# Diagnostic-to-action audit

The client receives edits through `Analysis::code_actions`, not diagnostic
`fixes` metadata. Published diagnostics intentionally resolve no edits. Therefore
handler-local fix tests alone do not prove an editor action exists.

| Diagnostic / request | Editor path and evidence | Limits / remaining work |
| --- | --- | --- |
| unused-variable | `quick_fix_for_diagnostic`; protocol apply/recheck and stale resolve passed | Local unused bindings; no claim for parameters |
| incomplete-match | Semantic exhaustiveness witnesses plus explicit wildcard placeholder; cross-file payload apply/recheck passed | `undefined` is unfinished user code; it is not an implementation of the branch |
| missing-fields (record expressions and patterns) | Inferred metadata through Semantics; named/anonymous expression and generic cross-file pattern apply/recheck passed | Patterns insert `_`, preserving match coverage |
| unresolved identifier / missing include | Semantic auto-include; real edits, client capabilities and stale resolution covered by server tests | Must not invent imports from spelling similarity |
| missing closing delimiter | Parser-token fix; brace/paren protocol apply/recheck passed | Missing semicolon has helper tests only; ordinary blocks accept omitted semicolons |
| type mismatch | No automatic coercion action | Dormant diagnostic metadata contains literal `expr` placeholders and unjustified int/nat coercions; cannot expose it as working fixes |
| duplicate definition | No diagnostic-specific client action | Deleting a declaration requires deliberate choice; dormant metadata is not evidence of safe removal |
| unreachable code | No diagnostic-specific client action | Requires statement-boundary edit and apply/recheck before exposure |
| mismatched argument count | Unresolved diagnostic metadata, no concrete diagnostic-specific action | Choosing/removing arguments cannot be inferred from count alone |
| effect violation | Unresolved diagnostic metadata | No universally safe annotation change |
| remove trailing return / unnecessary else | Diagnostic metadata; contextual refactors are separate | No diagnostic-specific round-trip claim yet |
| shadowed binding | Dormant handler metadata | Single-token replacement would leave references wrong; any future action must use rename semantics |
| unused import | No native producer found; unsafe source-action heuristic removed | Includes affect types, order and preprocessing, not only calls |
| unnecessary mutability | No native producer found | Dormant var-to-let helper is not an editor feature |
| deprecated syntax | No native producer found | Synthetic dispatch tests only |
| missing effect annotation | No native producer found | Synthetic dispatch tests only |

Associated source-action safety checks: include cleanup preserves source order
and repetition; formatting is labelled explicitly. The former removal heuristic
based on absence of calls is gone. Cross-file type-only dependency regression
passed (`target/editor-include-action-safety.log`); 130 assist tests passed
(`target/editor-assists-include-safety.log`).

Guarded and nested match witness tests passed (`target/editor-match-witness-final.log`).
Stale diagnostics supplied with a new request are revalidated against the current
snapshot (`target/editor-stale-client-diagnostic.log`); resolve still checks the
source fingerprint for later edits. Final current-tree default/no-SMT workspace gates passed (2388/2373 tests, 17 ignored each); strict corpus 276/276 and 217/217 also passed. Unsupported edits above must
not be counted as completed client fixes simply because a diagnostic handler
constructs an `Assist` value. Existing dormant metadata is technical debt; adding
new automatic conversions/annotation changes is not required to silence every
possible diagnostic.
