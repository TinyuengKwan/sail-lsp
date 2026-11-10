# Upstream compatibility checks

The main CI conclusion requires `corpus.yml`. It uses fixed Sail and RISC-V
revisions, recorded in that workflow, so an unrelated upstream change does not
change a PR's oracle. Its compiler is built unmodified; the optional
check-context patch is applied only after the stock-compiler comparisons.

`upstream.yml` separately checks the default branches of Sail, sail-riscv and
sail-arm every Monday and on manual invocation. It records the resolved commits
and binary hashes. It does not run on PRs. A semantic difference, unsupported
configuration, missing input, timeout or build failure fails that workflow and
retains a report; none is treated as a successful empty result. Before the
comparison is available, setup/build failures produce an infrastructure report
and retain the failed step's logs.

The project monitor compares ordered **file occurrences** (duplicates included),
all modules, each root, a pair of roots, and direct `requires` edges including
isolated module nodes. Both dependency-graph evaluators receive the same typed
variables. RISC-V runs with `RMEM=false` and `RMEM=true`. A small conditional
dependency fixture and a file path containing a space guard the oracle itself.
Monitor tests cover typed CLI limitations, malformed graphs, nonzero graph
process status, missing model sources and timeout handling.

Armv9.4-A is an additional real model, with its devices and no-devices source
configurations. sail-arm has no native project manifest. The monitor records
the Makefile hash and its ordered `SAIL_SRCS`, validates that every source
exists, then creates a temporary one-module wrapper pointing to those sources.
It fails on an unsupported Makefile expression rather than guessing the order.
These checks establish project loading/order only; they do not establish Arm
body typecheck parity. The independent typecheck, kind and mutation checks in
the pinned corpus workflow retain their own scopes.

The historical Armv9.3-A model is selectable with `--arm-model arm-v9.3-a`.
It is outside the default inventory because it uses `from` as an identifier,
which current Sail rejects while parsing its source. A selected source syntax
rejection is reported as `source_syntax_rejection` and fails the comparison.
It is never counted as a Sail-LSP project-semantic pass or a typecheck result.

Run the project monitor after building the stock compiler and bridge:

```sh
cargo build --release --locked -p project-model --example project_oracle
python3 scripts/test_upstream_monitor.py
python3 scripts/upstream_monitor.py \
  --compiler /path/to/sail/_build/default/src/bin/sail.exe \
  --lsp-oracle target/release/examples/project_oracle \
  --sail-src /path/to/sail --riscv /path/to/sail-riscv \
  --arm-src /path/to/sail-arm --output target/upstream/report.json
```

Keep the full report when investigating a difference. It includes manifest
text/hash, variables, roots, tool/source provenance, command arguments, working
directories, exit status and raw compiler output. `both_rejected` records both
errors for review; it asserts rejection agreement only, not equivalent reasons.
Boolean-looking string variables and other inputs that cannot round-trip
through the stock CLI are explicit `unsupported` results and make the run
inconclusive/failing. No coverage is silently removed to make a run pass.

When upgrading a fixed revision, first run the latest monitor, classify its
differences, and add focused regressions for confirmed semantic changes. Update
the pins and recorded expectations together, with the matching full corpus
results. The scheduled workflow's existence is not evidence that a remote run
has executed; local reports and later CI artifacts identify the actual runs.
