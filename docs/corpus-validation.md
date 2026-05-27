# Corpus Validation

sail-lsp is validated against [sail-riscv](https://github.com/riscv/sail-riscv),
a 158-file RISC-V ISA model written in Sail. This document describes the
validation methodology, tools, and results.

## Tool

```bash
# Single-file mode: each file in subprocess, crash-isolated
cargo run --example corpus_check -- /path/to/sail-riscv

# Workspace mode: cross-file type resolution (mirrors LSP server)
cargo run --example corpus_check -- --workspace /path/to/sail-riscv
```

The corpus checker (`examples/corpus_check.rs`) runs three stages per file:

1. **Parse** — `syntax::event_parser::parse_to_cst()` — detects syntax errors
2. **Name resolution** — `ItemTree::build_from_cst()` + `DefMap::build()` — detects duplicate/unresolved definitions
3. **Type inference** — `hir_ty::check_file()` — detects type mismatches, unresolved fields, arity errors

Single-file mode runs each file in a subprocess with 10-second timeout and
256MB stack thread for crash isolation. Workspace mode loads all files into
a single process and uses `check_file_with_workspace` for cross-file type
resolution.

## Results

| Category | Initial (2026-05-08) | Current (2026-05-27) | Reduction |
|----------|---------------------|----------------------|-----------|
| Parse errors | 140 | **0** | **-100%** |
| Name resolution | 414 | **0** | **-100%** |
| Type errors | 736 | **0** | **-100%** |
| Unused-var FP | — | **0** | — |
| Crashes | 6 | **0** | **-100%** |
| **Total issues** | **1,296** | **0** | **-100%** |

Remaining diagnostics (all true positives): 70 unused-variable warnings,
2 redundant-type-annotation, 2 unreachable-code.

Cross-file struct field accesses are resolved via `WorkspaceContext::cross_file_records`
in workspace mode. Both single-file and workspace modes report 0 issues.

### Benchmarks

Integrated benchmarks can be run with:

```bash
RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks
```

See [benchmarking.md](benchmarking.md) for full details.

## Fix Categories

### Parser Fixes (0 remaining from 140)

| Fix | Upstream Reference | Count |
|-----|-------------------|-------|
| `config` keyword + expression | `parser.mly:766` E_config | -8 |
| Scattered `clause` keyword | `parser.mly:1338` SD_enumcl/SD_funcl | -0 (DefMap) |
| `///` doc comments before definitions | RA `outer_attrs()` pattern | -37 (part) |
| `$[attr]` attribute pragmas | `lexer.mll:244-245` Attribute | -37 (part) |
| `private` visibility modifier | `parse_ast.ml:457` DEF_private | -37 (part) |
| Infix operators `<_s`, `<=_u` | `lexer.mll:186-189` operatorn | -42 |
| `[e with f=v]` struct update depth | `parser.mly:816-817` E_struct_update | -12 |
| Mapping guards `if...=>` | `parser.mly` mapping guard | -14 |
| Type ascription `(e:t)` | `parser.mly` Lparen exp Colon typ | -13 |
| `try/catch` syntax | `parser.mly` Try exp Catch | -6 |
| `forall` quantifier skip in Phase 1 | `parser.mly` function syntax | -12 |
| Wildcard `_` in struct pattern | Upstream struct pattern wildcards | -1 |

### Name Resolution Fixes (0 remaining from 414)

| Fix | Upstream Reference | Count |
|-----|-------------------|-------|
| Scattered clause → SCATTERED_CLAUSE_DEF | `parser.mly:1338` SD_enumcl | -414 |
| DefCollector: skip DuplicateDefinition for clauses | RA `ItemScope::define_impl` | (same) |

### Type Inference Fixes (0 remaining from 736)

| Fix | Upstream Reference | Count |
|-----|-------------------|-------|
| `config` expression purity | `type_check.ml:2387`; `effects.ml` | -566 |
| Env pollution from non-enum clauses | `parser.mly:1338` SD_enumcl distinction | -115 |
| `is_declared_pure` vs absent effects | `type_check.ml` + `effects.ml` | -155 |
| Bitfield `.bits` field desugaring | `bitfield.ml:96,104-108` | -52 |
| Unit param arity `f()≡f(())` | `type_check.ml:4107-4109` | -26 |
| `private` in `first_keyword_text` | `parse_ast.ml:457`; RA `opt_visibility` | -15 |
| Union variant IDENT type extraction | `type_check.ml:5029` Tu_ty_id | -5 (qualitative) |
| Setter desugaring `f(x)=v→f(x,v)` | `type_check.ml:3397-3400` LE_app | -2 |
| Metadata param override | `type_check.ml:4104` arity from type sig | -37 |
| Newtype constructor registration | `initial_check.ml:1888` TD_variant | +3 found |
| TUPLE_EXPR as union variant type | `parser.mly` Tu_ty_id | -6 |
| Cross-file records/bitfields merge | `type_env.ml` global records | LSP path |
| mk_synonym alias parameter substitution | `type_env.ml:849-876` mk_synonym | -22 |
| Suffix comparison ops → bool | RA `enforce_builtin_binop_types` | -5 |
| Bit-slice dynamic → fresh var | `type_check.ml:1763` bitvector_subrange | -2 |
| Block divergence → error type | RA `new_maybe_never_var` | -15 |
| bits ≡ bitvector equivalence | `type_env.ml` expand_synonyms | -1 |
| filter_overload_tree cross-file | `type_check.ml:1859-1932` | -6 |
| Dependent-match arm widening | Sail dependent type semantics | -10 |
| implicit(N) ↔ int(M) App subtype | `type_check.ml:1372` implicit_to_int | -1 |
| Non-block function body assignment | `type_check.ml:3397` E_assign | -5 |
| Bitfield field as vector index | `bitfield.ml` field_accessor_ids | -1 |
| Bitvector alias operator dispatch | `type_env.ml` expand_synonyms | -2 |

### Crash Fixes (0 remaining from 6)

| Fix | Upstream Reference | Count |
|-----|-------------------|-------|
| Remove check_named_binding_from_text | RA DefWithBodyId pattern | -3 |
| match_check complexity limit | RA `pat_analysis.rs:107` 500K | -1 |
| Type variable freshening | `type_check.ml:5295` Env.add_typquant | -1 |
| Scattered clause body parsing | (parser fix) | -1 |

## Debug Methodology

Each fix followed this process:

1. **Binary search** — truncate file at various line numbers to isolate the trigger
2. **Minimal reproduction** — extract smallest `.sail` file that reproduces the error
3. **eprintln tracing** — add targeted debug output to trace data flow
4. **Upstream verification** — read the corresponding upstream Sail compiler code to confirm the correct semantics
5. **RA comparison** — check how rust-analyzer handles the analogous construct
6. **Regression test** — run full corpus check before committing
