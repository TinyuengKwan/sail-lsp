# Editor performance regression gate

`performance.yaml`, called by main CI, builds the PR base (or previous push
revision) and candidate with the same Rust toolchain, release mode, and vendored
Z3. Each source uses its own Cargo target directory, preventing a library from
another worktree from being reused because of source timestamps. It alternates
baseline/candidate order between repetitions on the same runner and CPU affinity.
The main conclusion job requires this comparison.

The fixed policy is `benchmarks/editor-performance.json`. It generates an isolated
128-file, eight-functions-per-file Sail project, with 24 editing cycles per run
and five runs per binary. No developer model is modified.

The measured operations are:

* First **usable** completion after opening a fresh process, including loading,
  cancelled attempts and retries. This is not a cold OS filesystem cache claim.
* Completion during repeated semantic body changes.
* Completion after observing workspace reload begin, while reload traffic is active.
* Cancellation response latency, counting only actual RequestCancelled replies.
  A workload with no successful cancellations is inconclusive and fails.
* Diagnostic convergence: each cycle first requires a known argument type error,
  then repairs it and requires a current full response without errors. An old
  empty result cannot satisfy both phases.

Each run retains raw protocol responses, events and latency samples. Comparison
uses the median of each run's p95 and p99 for every operation. First completion
has one observation per process, so these percentiles represent the repeated
fresh-process observation, not a within-process tail distribution. The fixed
policy permits a 1.5 regression ratio plus 15 ms of scheduling noise (100 ms for
startup). It detects large regressions; it is not a claim to detect every small
optimization on shared runners. Change these reviewable policy values only with
same-host control measurements, never to conceal a failing implementation.

A unique experiment ID, environment, workload, binary hashes and build metadata
are recorded. Results from different experiment IDs, environments or workloads
cannot be compared. Benchmark failures, incomplete samples, protocol errors,
server crashes and missing base revisions fail the job instead of reducing scope.

Run a comparison locally after building both versions:

```sh
python3 scripts/performance_gate.py \
  --baseline-server /absolute/path/to/baseline-sail-lsp \
  --server target/release/sail-lsp \
  --cpu-affinity 0,1 --output target/performance-results
```

Use available CPUs from `os.sched_getaffinity(0)`, rather than assuming CPUs 0/1
are allowed. Both binaries must use matching build modes and solver linkage;
the CI build commands enforce that. Raw inputs/results, policy and comparison
are uploaded on failures as well as successes. Real-model probes in the pinned
corpus workflow complement this fixed synthetic regression gate; neither implies
rust-analyzer performance parity or a cross-machine latency guarantee.

## Pinned real-model absolute targets

`corpus.yml` also runs `scripts/real_model_gate.py` against the pinned sail-riscv
and sail-arm revisions in `benchmarks/real-model-performance.json`, with the
pinned Sail standard library. Model revisions and clean source checkouts are
required. Arm has no native manifest: the existing ordered `SAIL_SRCS` Makefile
adapter is used for arm-v9.4-a and `devices.sail`. Its manifest, ordered sources
and Makefile hash are recorded. The gate changes only unsaved document overlays.

Three fresh server runs per model measure the first usable real-symbol
completion, completion/hover/definition after body edits, completion alongside
workspace search and cancelled references, completion after reload begins, and
diagnostic repair after first observing a known error in an appended function.
Existing model diagnostics outside that function do not satisfy the error check
or prevent a repaired function from converging. Native background diagnostics
remain enabled; external compiler Flycheck is disabled. Cache priming finishes
before warm samples. First completion permits warm OS caches.

The large-document workload edits the full vector arithmetic instruction file in
RISC-V, plus Arm's `instrs64.sail` and 8 MB `sysregs_autogen.sail`. Minimum source
sizes and project membership are checked. Uniquely named appended functions
give exact expected parameter types, binding hints and name ranges while
retaining the original model contents. Each cycle checks signature help,
viewport inlay hints, full references and rename edits. Rename is applied to
the unsaved overlay, then references and definition are checked at the new name;
wrong ranges, stale versions and incomplete edits fail the gate. First use of
each large document has its own cold-interaction target; subsequent body-edit
completion, signature, inlay, references, rename and requery have independent
limits.

The policy defines absolute p95/p99 millisecond targets, including worker queue
time separately from client-observed latency. Every repetition must pass; missing
metrics, insufficient actual cancellations, missing real-symbol results, crashes,
unlinked sources and protocol failures fail the gate. Raw responses, events,
queue logs, binary hashes, build identity and host/affinity are retained under
the output directory. A fresh process has one startup sample per run; its p95/p99
are that observation, not an estimate of a startup tail distribution.

```sh
python3 scripts/real_model_gate.py --server target/release/sail-lsp \
  --riscv /path/to/pinned/sail-riscv --arm /path/to/pinned/sail-arm \
  --sail-src /path/to/clean/pinned/sail --output target/real-model-performance
```

These are initial project-specific targets, not measurements of rust-analyzer
or a cross-machine performance promise. Review results on the recorded host;
investigate failures before changing targets. This absolute gate complements
the independent alternating synthetic regression comparison above.

## Request tracing and local investigations

For an editor-style burst, enable the latency tracing target and summarize stderr:

```sh
SAIL_LOG=sail_lsp::latency=debug python3 scripts/editor_probe.py \
  --server target/release/sail-lsp --workspace /path/to/sail-riscv \
  --cycles 20 --burst --output target/editor-burst.json
python3 scripts/latency_summary.py target/editor-burst.stderr.log
```

Worker samples include retries. Server request totals begin when the main loop
receives the request; the stdio workload also includes transport and unread
messages. Compare repeated runs on the same host. A pipelined burst is a stress
workload, not ordinary typing latency.
`preprocess_events` and `preprocess_content` samples identify per-file projection
costs by file ID and byte size, separating them from aggregate `apply_change`
plan time. Raw logs preserve these IDs even when the summary groups durations.
`hover_phases` separates visibility, markup and supplemental resolution;
`hover_render_phases` divides markup into resolution, signature, annotation and
effect work. These distinguish a first-query cache miss from worker queue delay.

For query reuse, parsing and cache-retention investigations, use the tools in
[contributor validation](../CONTRIBUTING.md#validation). `incremental_stats`
accepts `--profile` for query timing and allocation observations. Execution
instrumentation adds overhead; use ordinary runs for latency comparisons.
