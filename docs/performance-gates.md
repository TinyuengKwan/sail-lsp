# Editor performance regression gate

`performance.yaml`, called by main CI, builds the PR base (or previous push
revision) and candidate with the same Rust toolchain, release mode, and vendored
Z3. It alternates baseline/candidate order between repetitions on the same
runner and CPU affinity. The main conclusion job requires this comparison.

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
