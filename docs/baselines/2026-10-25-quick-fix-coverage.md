# Diagnostic-to-action audit

Updated 2026-10-29 against the working tree recorded in
[the P2/P3 baseline](2026-10-29-editor-p2-p3.json). Earlier statements that
`current.fixes` never reaches clients, or that no conversion action exists,
are superseded by this audit.

`Analysis::code_actions` revalidates client diagnostics against the current
snapshot, consumes concrete `current.fixes`, and omits unresolved metadata
without a `SourceChange`. `codeAction/resolve` rejects stale source fingerprints.
A handler's synthetic dispatch test alone does not establish editor support.

| Diagnostic / request | Current client behavior and evidence | Limits |
| --- | --- | --- |
| unused-variable | Offer → resolve → apply → re-diagnose; `binding_quick_fixes_remove_their_diagnostic_after_application` | Local unused bindings; no claim for unused parameters |
| incomplete-match | Cross-file constructor payload round trips in `match_fixes_apply_and_recheck_cross_file_constructor_payloads` | Inserted `undefined` is a user implementation placeholder |
| missing-fields | Named/anonymous expressions and generic patterns round-trip in the record-fill protocol tests | Patterns insert `_`; expressions may insert placeholders |
| unresolved identifier / missing include | Semantic auto-include round trip and stale resolve tests | Requires a real visible declaration and supported client edit capabilities |
| missing closing delimiter | Brace/parenthesis EOF round trips in `closing_delimiter_quick_fixes_apply_at_eof` | Missing semicolon has helper tests only; not claimed as a protocol round trip |
| type mismatch: bits → int | `unsigned` and `signed` offered when a compatible declared callable shape is visible; `conversion_quick_fixes_apply_and_recheck_over_the_protocol` and `quick_fix_tests` apply and recheck | No automatic choice of signedness. Absent, locally shadowed or wrong-shape callables are excluded. Custom callable constraints still require normal checking |
| other type mismatches | No conversion action | bool/bit, int/nat, constrained numeric destination, width changes and arbitrary casts are unsupported |
| unreachable code | Concrete removal reaches client; binding protocol round trip plus multi-statement IDE apply/recheck | An individual action removes its target; other unreachable statements may remain |
| duplicate definition | No automatic rename action | Audit removed the unsafe whole-node replacement with `name_2`; resolving references and collisions requires semantic rename |
| shadowed binding | No automatic rename action | Removed the single-node prefix rename, which could leave references bound to a different variable |
| mismatched argument count | Unresolved metadata omitted by client action collection | Cannot infer missing arguments or safely discard extra arguments/effects |
| effect violation | Unresolved metadata omitted | No generally safe annotation or behavior change |
| trailing return / unnecessary else | No diagnostic-specific concrete action; contextual refactors are separate | Unresolved metadata is not a client fix |
| unused import | No native producer found; unsafe source-action heuristic remains removed | Includes also affect types, order and preprocessing |
| unnecessary mutability / deprecated syntax / missing effect annotation | Handler dispatch scaffolding only; no native producer found | Not counted as supported client fixes |

The `ide` quick-fix fixture now uses an explicit ordered project and include
expansion, so availability checks exercise `Semantics` source scope rather than
an unconfigured flat library environment. Tests also cover missing conversion
functions, incompatible callable shapes and constrained destinations.

Relevant protocol coverage is in `crates/sail-lsp/src/server_check.rs`; IDE
coverage is in `crates/ide/src/quick_fix_tests.rs`. Gate results and corpus
provenance belong to the linked baseline, rather than copied historic totals.
