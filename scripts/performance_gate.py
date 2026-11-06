#!/usr/bin/env python3
"""Compare repeated real stdio workloads on one host; correctness failures are fatal.

First usable completion includes loading and retries. Each edit waits for current
usable completion and repaired diagnostics. Reload traffic is issued only after
observing reload progress. Cancellations require actual RequestCancelled replies.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time
import uuid

from editor_probe import Client

ROOT = Path(__file__).resolve().parent.parent
METRICS = ("first_completion", "typing_completion", "scan_completion", "cancel_response", "diagnostic_settle")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def version(binary):
    result = subprocess.run([str(binary), "--version", "--json"], text=True, capture_output=True, timeout=10)
    try:
        return json.loads(result.stdout)
    except ValueError:
        result = subprocess.run([str(binary), "--version"], text=True, capture_output=True, timeout=10, check=True)
        return {"version": result.stdout.strip(), "metadataUnavailable": True}


def fingerprint(affinity):
    return {"experimentId": str(uuid.uuid4()), "os": platform.system(), "arch": platform.machine(), "kernel": platform.release(),
            "python": platform.python_version(), "logicalCpus": os.cpu_count(),
            "hostAffinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
            "serverAffinity": affinity}


def quantile(values, probability):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * probability) - 1)]


def generate(root, workload):
    files = ["main.sail"]
    for index in range(workload["files"]):
        name = f"part_{index:04}.sail"
        source = "".join(f"function part_{index}_f{f}(v : int) -> int = v\n" for f in range(workload["functionsPerFile"]))
        (root / name).write_text(source)
        files.append(name)
    (root / "sail.proj").write_text("Main { files " + ", ".join(files) + " }\n")
    source = "function target_identity(v : int) -> int = v\nfunction edit_me(v : int) -> int = target_identity(v)\n"
    (root / "main.sail").write_text(source)
    return source


def at(source):
    offset = source.rindex("target_id") + len("target_id")
    prefix = source[:offset]
    return {"line": prefix.count("\n"), "character": len(prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2}


def completion_has_target(response):
    result = response.get("result")
    items = result.get("items", []) if isinstance(result, dict) else result or []
    return any(item["label"] == "target_identity" for item in items)


def usable(client, method, params, phase, predicate=lambda response: True):
    began = time.monotonic()
    deadline = began + 30
    while time.monotonic() < deadline:
        response = client.ask(method, params, phase, timeout=30)
        error = response.get("error")
        if error and error["code"] not in (-32801, -32802):
            raise AssertionError(f"{phase}: unexpected error: {response}")
        if not error and predicate(response):
            return round((time.monotonic() - began) * 1000, 3)
        client.drain(.01)
    raise TimeoutError(f"{phase}: no usable response")


def latest_status(client):
    return next((e["params"] for e in reversed(client.events) if e.get("method") == "sail-lsp/serverStatus"), None)


def wait_ready(client, events_after=0):
    deadline = time.monotonic() + 60
    while not any((e.get("method") == "sail-lsp/serverStatus" and e["params"]["quiescent"])
                  or e.get("params", {}).get("message") == "sail-lsp: ready"
                  for e in client.events[events_after:]):
        if time.monotonic() > deadline:
            raise TimeoutError("workspace did not become ready")
        client.pump(.02)


def measure(binary, policy, output, affinity):
    workload = policy["workload"]
    metrics = {key: [] for key in METRICS}
    with tempfile.TemporaryDirectory(prefix="sail-perf-gate-") as temporary:
        root = Path(temporary)
        source = generate(root, workload)
        document = {"uri": (root / "main.sail").as_uri()}
        with output.with_suffix(".stderr.log").open("w") as stderr:
            client = Client(binary, stderr, affinity)
            try:
                initialized = client.ask("initialize", {
                    "processId": os.getpid(), "workspaceFolders": [{"uri": root.as_uri(), "name": "performance-gate"}],
                    "initializationOptions": {"flycheck": {"enable": False}, "workspace": {"primeCachesNumThreads": 2}},
                    "capabilities": {"window": {"workDoneProgress": True}, "textDocument": {"diagnostic": {}}},
                }, "initialize")
                if not initialized.get("result"): raise AssertionError(initialized)
                client.send("initialized", {})
                began = time.monotonic()
                client.send("textDocument/didOpen", {"textDocument": {**document, "languageId": "sail", "version": 1, "text": source}})
                usable(client, "textDocument/completion", {"textDocument": document, "position": at(source)}, "first-completion", completion_has_target)
                metrics["first_completion"].append((time.monotonic() - began) * 1000)
                wait_ready(client)
                if not client.prime_completions:
                    client.wait_for_priming(0)
                current_version = 1
                for cycle in range(workload["cycles"]):
                    current_version += 1
                    edited = source.replace("target_identity(v)\n", "target_identity(v + 1)\n") if cycle % 2 else source
                    client.send("textDocument/didChange", {"textDocument": {**document, "version": current_version}, "contentChanges": [{"text": edited}]})
                    metrics["typing_completion"].append(usable(client, "textDocument/completion", {"textDocument": document, "position": at(edited)}, "typing", completion_has_target))
                    # A fresh known error followed by a repair proves diagnostics
                    # actually converge; an old empty result cannot pass this.
                    broken = source.replace("target_identity(v)\n", "target_identity(true)\n")
                    current_version += 1
                    client.send("textDocument/didChange", {"textDocument": {**document, "version": current_version}, "contentChanges": [{"text": broken}]})
                    def diagnostics(response, has_error):
                        result = response.get("result", {})
                        if result.get("kind") != "full": return False
                        errors = [d for d in result.get("items", []) if d.get("severity") == 1]
                        return bool(errors) == has_error
                    usable(client, "textDocument/diagnostic", {"textDocument": document}, "error-diagnostics", lambda r: diagnostics(r, True))
                    current_version += 1
                    began = time.monotonic()
                    client.send("textDocument/didChange", {"textDocument": {**document, "version": current_version}, "contentChanges": [{"text": edited}]})
                    usable(client, "textDocument/diagnostic", {"textDocument": document}, "repaired-diagnostics", lambda r: diagnostics(r, False))
                    metrics["diagnostic_settle"].append((time.monotonic() - began) * 1000)
                    request_id = client.send("textDocument/references", {"textDocument": document, "position": at(edited), "context": {"includeDeclaration": True}}, request=True)
                    began = time.monotonic()
                    client.send("$/cancelRequest", {"id": request_id})
                    response = client.wait_response(request_id, "textDocument/references", "cancel", began, timeout=30)
                    if response.get("error", {}).get("code") == -32800:
                        metrics["cancel_response"].append((time.monotonic() - began) * 1000)
                    elif response.get("error", {}).get("code") not in (None, -32801, -32802):
                        raise AssertionError(response)
                    event_start = len(client.events)
                    client.send("sail-lsp/reloadWorkspace", None)
                    deadline = time.monotonic() + 60
                    def reload_begin(event):
                        return event.get("method") == "$/progress" and event.get("params", {}).get("value", {}).get("kind") == "begin" and event["params"]["value"].get("title") == "Reloading workspace"
                    while not any(reload_begin(e) for e in client.events[event_start:]):
                        if time.monotonic() > deadline: raise TimeoutError("reload never started")
                        client.pump(.01)
                    # Ignore previously queued status messages preceding this
                    # specific begin; short scans may never change serverStatus.
                    reload_index = next(i for i in range(event_start, len(client.events)) if reload_begin(client.events[i]))
                    metrics["scan_completion"].append(usable(client, "textDocument/completion", {"textDocument": document, "position": at(edited)}, "during-scan", completion_has_target))
                    wait_ready(client, reload_index + 1)
                minimum = policy["comparison"]["minimumCancelledResponses"]
                if len(metrics["cancel_response"]) < minimum:
                    raise AssertionError("Insufficient genuinely cancelled requests; fixture is inconclusive")
                client.ask("shutdown", None, "shutdown")
                client.send("exit", None)
                client.process.wait(timeout=15)
                if client.process.returncode != 0: raise AssertionError("Server did not exit successfully")
            finally:
                if client.process.poll() is None: client.process.kill(); client.process.wait()
                client.reader.join(timeout=2)
                output.write_text(json.dumps({"metrics": metrics, "samples": client.samples, "events": client.events}, indent=2) + "\n")
    return metrics


def compare(baseline, candidate, policy):
    if baseline["environment"] != candidate["environment"]: raise ValueError("Different experiments/environments cannot be compared")
    if baseline["workload"] != candidate["workload"]: raise ValueError("Different workloads cannot be compared")
    if not (len(baseline["runs"]) == len(candidate["runs"]) >= 3): raise ValueError("At least three matched runs are required")
    comparisons = []
    for metric in METRICS:
        for percentile, probability in [("p95", .95), ("p99", .99)]:
            before = statistics.median(quantile(r[metric], probability) for r in baseline["runs"])
            after = statistics.median(quantile(r[metric], probability) for r in candidate["runs"])
            allowance = policy["comparison"]["firstCompletionNoiseFloorMs" if metric == "first_completion" else "noiseFloorMs"]
            limit = before * policy["comparison"]["maximumRegressionRatio"] + allowance
            comparisons.append({"metric": metric, "percentile": percentile, "baseline_ms": before, "candidate_ms": after, "limit_ms": limit, "passed": after <= limit})
    return {"status": "passed" if all(c["passed"] for c in comparisons) else "failed", "comparisons": comparisons}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--server", type=Path, required=True)
    p.add_argument("--baseline-server", type=Path, required=True)
    p.add_argument("--policy", type=Path, default=ROOT / "benchmarks/editor-performance.json")
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--cpu-affinity")
    args = p.parse_args()
    policy = json.loads(args.policy.read_text())
    if policy["schemaVersion"] != 1 or policy["workload"]["repeats"] < 3:
        raise ValueError("A versioned policy with at least three repetitions is required")
    if any(policy["workload"][key] <= 0 for key in ["files", "functionsPerFile", "cycles"]):
        raise ValueError("Workload sizes must be positive")
    if policy["comparison"]["maximumRegressionRatio"] < 1 or policy["comparison"]["minimumCancelledResponses"] < 1:
        raise ValueError("Invalid comparison policy")
    args.output.mkdir(parents=True, exist_ok=True)
    env = fingerprint(args.cpu_affinity)
    reports = {name: {"environment": env, "workload": policy["workload"], "serverSha256": digest(binary), "build": version(binary), "runs": []} for name, binary in [("baseline", args.baseline_server.resolve()), ("candidate", args.server.resolve())]}
    report = {"status": "running", "policy": policy}
    try:
        for repeat in range(policy["workload"]["repeats"]):
            for name in (("baseline", "candidate") if repeat % 2 == 0 else ("candidate", "baseline")):
                binary = args.baseline_server.resolve() if name == "baseline" else args.server.resolve()
                metrics = measure(binary, policy, args.output / f"{name}-{repeat}.json", args.cpu_affinity)
                reports[name]["runs"].append(metrics)
                print(json.dumps({"server": name, "repeat": repeat, "metrics": {k: {"count": len(v), "p95_ms": quantile(v, .95)} for k, v in metrics.items()}}), flush=True)
        report = {**compare(reports["baseline"], reports["candidate"], policy), "policy": policy}
    except BaseException as error:
        report = {"status": "failed", "error": repr(error), "policy": policy}
        raise
    finally:
        for name, data in reports.items():
            (args.output / f"{name}.json").write_text(json.dumps(data, indent=2) + "\n")
        (args.output / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")
    if report["status"] != "passed":
        raise SystemExit("Editor performance regression: inspect comparison.json and raw stdio samples")


if __name__ == "__main__": main()
