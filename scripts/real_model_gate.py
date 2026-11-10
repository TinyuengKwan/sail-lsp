#!/usr/bin/env python3
"""Gate pinned real-model interactions and retain raw stdio/queue samples.

Only unsaved editor overlays are changed. Arm uses the ordered Makefile adapter;
neither model checkout is modified. Failures and missing data never pass.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from urllib.parse import unquote, urlparse

from editor_probe import Client
from performance_gate import digest, fingerprint, quantile, usable, version, wait_ready
from upstream_monitor import arm_adapter, provenance

ROOT = Path(__file__).resolve().parent.parent
INTERACTIVE = {"textDocument/completion", "textDocument/hover", "textDocument/definition",
               "textDocument/signatureHelp", "textDocument/inlayHint"}
METRICS = {"first_completion", "completion", "hover", "definition", "mixed_completion",
           "reload_completion", "diagnostic_settle", "cancel_response", "interactive_queue",
           "first_large_interaction", "large_edit_completion", "signature_help", "inlay_hints",
           "references", "rename", "rename_requery"}
PROBE = "\nfunction __sail_perf_identity(v : int) -> int = v\nfunction __sail_perf_edit(v : int) -> int = __sail_perf_identity(v)\n"


def validate_policy(policy):
    if policy["schemaVersion"] != 2 or policy["repeats"] < 3 or policy["cycles"] < 8:
        raise ValueError("Versioned policy requires three runs and at least eight cycles")
    if not 1 <= policy["reloadEvery"] <= policy["cycles"]:
        raise ValueError("Reload traffic is required in every run")
    if not 1 <= policy["minimumCancelledResponses"] <= policy["cycles"]:
        raise ValueError("Actual cancellation observations are required")
    if set(policy["models"]) != {"riscv", "arm"} or set(policy["limitsMs"]) != METRICS:
        raise ValueError("Both pinned models and every required interaction metric need limits")
    if policy.get("renameEditFormat") != "versionedDocumentChanges":
        raise ValueError("Applied rename traffic must exercise versioned document edits")
    for name, model in policy["models"].items():
        large = model.get("largeDocuments", [])
        roles = {item["role"] for item in large}
        if "instruction" not in roles or (name == "arm" and "generated" not in roles):
            raise ValueError("Large instruction files and Arm generated files are required")
        if any(item.get("minimumBytes", 0) < 50_000 for item in large):
            raise ValueError("Large-file fixtures cannot silently become small probes")
    for limits in policy["limitsMs"].values():
        if set(limits) != {"p95", "p99"} or any(not math.isfinite(v) or v <= 0 for v in limits.values()):
            raise ValueError("Positive finite p95 and p99 limits are required")
        if limits["p99"] < limits["p95"]:
            raise ValueError("The p99 limit cannot be smaller than p95")


def source_fingerprint():
    """Include uncommitted and untracked build inputs, excluding target caches."""
    paths = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT).split(b"\0")
    result = hashlib.sha256()
    for raw in sorted(set(paths)):
        if not raw:
            continue
        name = os.fsdecode(raw)
        if not (name.startswith(("crates/", "lib/")) or name in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"}):
            continue
        path = ROOT / name
        result.update(raw + b"\0")
        result.update(hashlib.sha256(path.read_bytes()).digest() if path.is_file() else b"missing")
    return result.hexdigest()


def first_usable(client, params, symbol):
    # Startup has its own sixty-second budget; keep ordinary request retries
    # on the independent thirty-second failure limit used by the paired gate.
    began = time.monotonic()
    while time.monotonic() - began < 60:
        response = client.ask("textDocument/completion", params, "first-completion", timeout=60)
        if has_symbol(response, symbol):
            return
        if response.get("error", {}).get("code") not in (None, -32801, -32802):
            raise AssertionError(response)
        client.drain(.05)
    raise TimeoutError("first real-symbol completion exceeded 60s")


def position(source, offset):
    prefix = source[:offset]
    return {"line": prefix.count("\n"), "character": len(prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2}


def offset_at(source, at):
    lines = source.splitlines(keepends=True)
    if at["line"] == len(lines) and at["character"] == 0:
        return len(source)
    prefix = sum(len(line) for line in lines[:at["line"]])
    encoded = lines[at["line"]].encode("utf-16-le")
    return prefix + len(encoded[:at["character"] * 2].decode("utf-16-le"))


def same_document(uri, path):
    return Path(unquote(urlparse(uri).path)).resolve() == path.resolve()


def symbol_ranges(source, symbol):
    return [{"start": position(source, match.start()), "end": position(source, match.end())}
            for match in re.finditer(r"\b" + re.escape(symbol) + r"\b", source)]


def valid_references(response, path, source, symbol):
    locations = response.get("result")
    if not isinstance(locations, list) or len(locations) != 2:
        return False
    expected = symbol_ranges(source, symbol)
    actual = [location["range"] for location in locations if same_document(location["uri"], path)]
    return sorted(actual, key=str) == sorted(expected, key=str)


def apply_rename(response, path, source, symbol, replacement, version):
    edit = response.get("result")
    if not isinstance(edit, dict):
        raise AssertionError(response)
    changes = list(edit.get("changes", {}).items())
    for document in edit.get("documentChanges", []):
        if "textDocument" not in document:
            raise AssertionError(f"Unexpected resource operation: {document}")
        target = document["textDocument"]
        if target.get("version") != version:
            raise AssertionError(f"Stale rename version: {target}")
        changes.append((target["uri"], document["edits"]))
    edits = []
    for uri, replacements in changes:
        if not same_document(uri, path):
            raise AssertionError(f"Rename escaped the fixture: {uri}")
        for item in replacements:
            begin, end = (offset_at(source, item["range"][part]) for part in ("start", "end"))
            if source[begin:end] != symbol or item["newText"] != replacement:
                raise AssertionError(f"Rename range/text is incorrect: {item}")
            edits.append((begin, end, replacement))
    if len(edits) != 2 or len({(begin, end) for begin, end, _ in edits}) != 2:
        raise AssertionError(f"Rename must edit declaration and call exactly once: {edits}")
    for begin, end, text in sorted(edits, reverse=True):
        source = source[:begin] + text + source[end:]
    return source


def large_interactions(client, fixture, cycle, metrics, warmup=False):
    path, original, symbol, version = fixture["path"], fixture["original"], fixture["symbol"], fixture["version"]
    # Edit only the appended probe body, retaining the original model verbatim.
    source = original + (f"\nfunction {symbol}(argument : int) -> int = argument + {cycle % 2}\n"
                         f"function {symbol}_use(argument : int) -> int = {{\n"
                         f"  let observed = {symbol}(argument);\n  observed\n}}\n")
    document = {"uri": path.as_uri()}
    def change(text):
        nonlocal version
        version += 1
        client.send("textDocument/didChange", {"textDocument": {**document, "version": version}, "contentChanges": [{"text": text}]})
    def record(metric, action):
        began = time.monotonic()
        result = action()
        if not warmup:
            metrics[metric].append((time.monotonic() - began) * 1000)
        return result
    change(source)
    call = source.rindex(f"{symbol}(argument)")
    at = {"textDocument": document, "position": position(source, call + 1)}
    complete = {"textDocument": document, "position": position(source, call + len(symbol))}
    record("large_edit_completion", lambda: usable(client, "textDocument/completion", complete, "large-edit-completion", lambda r: has_symbol(r, symbol)))
    signature = {"textDocument": document, "position": position(source, call + len(symbol) + 1)}
    def has_signature(response):
        result = response.get("result") or {}
        signatures = result.get("signatures", [])
        active = result.get("activeSignature", 0)
        return (active < len(signatures) and symbol in signatures[active].get("label", "")
                and "int" in signatures[active]["label"] and len(signatures[active].get("parameters", [])) == 1
                and result.get("activeParameter", signatures[active].get("activeParameter", 0)) == 0)
    record("signature_help", lambda: usable(client, "textDocument/signatureHelp", signature, "large-signature", has_signature))
    binding = source.rindex("let observed")
    hint_position = position(source, binding + len("let observed"))
    hint_range = {"start": position(source, binding), "end": position(source, binding + len("let observed"))}
    def has_hint(response):
        def label(hint):
            value = hint.get("label", "")
            return value if isinstance(value, str) else "".join(part["value"] for part in value)
        return any(hint.get("position") == hint_position and "int" in label(hint) for hint in response.get("result") or [])
    record("inlay_hints", lambda: usable(client, "textDocument/inlayHint", {"textDocument": document, "range": hint_range}, "large-viewport-inlay", has_hint))
    reference_params = {**at, "context": {"includeDeclaration": True}}
    record("references", lambda: usable(client, "textDocument/references", reference_params, "large-full-references", lambda r: valid_references(r, path, source, symbol)))
    renamed = symbol + "_renamed"
    response = record("rename", lambda: client.ask("textDocument/rename", {**at, "newName": renamed}, "large-rename", timeout=30))
    if "documentChanges" not in (response.get("result") or {}):
        raise AssertionError(f"Versioned rename did not return documentChanges: {response}")
    source = apply_rename(response, path, source, symbol, renamed, version)
    change(source)
    renamed_call = source.rindex(f"{renamed}(argument)")
    renamed_params = {"textDocument": document, "position": position(source, renamed_call + 1)}
    def requery():
        usable(client, "textDocument/references", {**renamed_params, "context": {"includeDeclaration": True}}, "renamed-references", lambda r: valid_references(r, path, source, renamed))
        expected = symbol_ranges(source, renamed)[0]
        def definition_matches(response):
            locations = response.get("result") or []
            if isinstance(locations, dict):
                locations = [locations]
            return any(same_document(item.get("uri", item.get("targetUri", "")), path)
                       and item.get("range", item.get("targetSelectionRange")) == expected for item in locations)
        usable(client, "textDocument/definition", renamed_params, "renamed-definition", definition_matches)
    record("rename_requery", requery)
    fixture["version"] = version


def has_symbol(response, symbol):
    result = response.get("result")
    items = result.get("items", []) if isinstance(result, dict) else result or []
    return any(item.get("label") == symbol for item in items)


def queue_samples(text):
    result = []
    for line in text.splitlines():
        if "sail_lsp::latency" not in line or "request_worker" not in line:
            continue
        method = re.search(r"\bmethod=(\S+)", line)
        queued = re.search(r"\bqueue_us=(\d+)", line)
        if method and queued and method[1] in INTERACTIVE:
            result.append(int(queued[1]) / 1000)
    return result


def evaluate(runs, policy):
    checks = []
    for metric, limits in policy["limitsMs"].items():
        # Evaluate every run: a good run cannot conceal a failing one.
        for index, run in enumerate(runs):
            values = run.get(metric, [])
            for percentile, limit in limits.items():
                observed = quantile(values, int(percentile[1:]) / 100) if values else None
                checks.append({"run": index, "metric": metric, "percentile": percentile,
                               "observedMs": observed, "limitMs": limit,
                               "passed": observed is not None and observed <= limit})
    return checks


def check_pin(path, expected):
    info = provenance(path, 30)
    if info.get("revision") != expected or info.get("tracked_changes"):
        raise ValueError(f"Expected clean pinned model {expected}: {info}")
    # Unknown untracked source files could affect includes; do not ignore them.
    untracked = subprocess.check_output(["git", "-C", str(path), "ls-files", "--others", "--exclude-standard"], text=True)
    if untracked.strip():
        raise ValueError(f"Untracked model files in {path}: {untracked}")
    return info


def measure(server, root, manifest, model, policy, output, affinity):
    source_path = root / model["file"]
    original = source_path.read_text()
    source = original + PROBE
    symbol = model.get("symbol", model["needle"])
    offset = source.index(model["needle"])
    completion = position(source, offset + model["prefixLength"])
    at = position(source, offset + 1)
    document = {"uri": source_path.as_uri()}
    params = {"textDocument": document, "position": completion}
    symbol_params = {"textDocument": document, "position": at}
    metrics = {name: [] for name in policy["limitsMs"]}
    diagnostic_line = source[:source.rindex("function __sail_perf_edit")].count("\n")
    log_path = output.with_suffix(".stderr.log")
    with log_path.open("w") as stderr:
        client = Client(server, stderr, affinity)
        current_version = 1
        def change(text):
            nonlocal current_version
            current_version += 1
            client.send("textDocument/didChange", {"textDocument": {**document, "version": current_version},
                                                   "contentChanges": [{"text": text}]})
        def target_error(response, expected):
            result = response.get("result", {})
            errors = [d for d in result.get("items", []) if d.get("severity") == 1
                      and d["range"]["start"]["line"] >= diagnostic_line]
            return result.get("kind") == "full" and bool(errors) == expected
        try:
            initialized = client.ask("initialize", {
                "processId": os.getpid(), "workspaceFolders": [{"uri": root.as_uri(), "name": "real-model-gate"}],
                "initializationOptions": {"flycheck": {"enable": False}, "diagnostics": {"enable": True},
                                          "workspace": {"projectFile": str(manifest), "primeCachesNumThreads": 2}},
                "capabilities": {"window": {"workDoneProgress": True}, "textDocument": {"diagnostic": {}},
                                 "workspace": {"workspaceEdit": {"documentChanges": True}}},
            }, "initialize")
            if not initialized.get("result"):
                raise AssertionError(initialized)
            client.send("initialized", {})
            began = time.monotonic()
            client.send("textDocument/didOpen", {"textDocument": {**document, "languageId": "sail", "version": 1, "text": source}})
            first_usable(client, params, symbol)
            metrics["first_completion"].append((time.monotonic() - began) * 1000)
            wait_ready(client)
            # An early, empty workspace can finish priming before the real
            # project is installed. Its old completion must not bypass a
            # later priming wave that is still running.
            client.wait_for_priming(0)
            project = client.ask("sail-lsp/openProjectManifest", {"textDocument": document}, "project-membership")
            if not project.get("result"):
                raise AssertionError(f"Real source was not linked to its loaded project: {project}")
            contexts = client.ask("sail-lsp/includeContexts", document, "active-project-context")
            if not isinstance(contexts.get("result"), list) or not contexts["result"]:
                raise AssertionError(f"Real source has no active semantic project context: {contexts}")
            large_documents = []
            for index, item in enumerate(model["largeDocuments"]):
                path = root / item["file"]
                text = path.read_text()
                if len(text.encode()) < item["minimumBytes"]:
                    raise AssertionError(f"Large source unexpectedly shrank: {path}")
                fixture = {"path": path, "original": text, "symbol": f"__sail_perf_large_{index}", "version": 1}
                began = time.monotonic()
                client.send("textDocument/didOpen", {"textDocument": {"uri": path.as_uri(), "languageId": "sail", "version": 1, "text": text}})
                linked = client.ask("sail-lsp/openProjectManifest", {"textDocument": {"uri": path.as_uri()}}, "large-project-membership")
                if not linked.get("result"):
                    raise AssertionError(f"Large fixture is not part of the model: {path}: {linked}")
                contexts = client.ask("sail-lsp/includeContexts", {"uri": path.as_uri()}, "large-active-context")
                if not isinstance(contexts.get("result"), list) or not contexts["result"]:
                    raise AssertionError(f"Large fixture is inactive: {path}: {contexts}")
                large_interactions(client, fixture, 0, metrics, warmup=True)
                metrics["first_large_interaction"].append((time.monotonic() - began) * 1000)
                large_documents.append(fixture)
            for cycle in range(policy["cycles"]):
                edited = source.replace("__sail_perf_identity(v)\n", f"__sail_perf_identity(v + {cycle % 2})\n")
                change(edited)
                for name, method, arguments, predicate in [
                    ("completion", "textDocument/completion", params, lambda r: has_symbol(r, symbol)),
                    ("hover", "textDocument/hover", symbol_params, lambda r: bool(r.get("result"))),
                    ("definition", "textDocument/definition", symbol_params, lambda r: bool(r.get("result"))),
                ]:
                    metrics[name].append(usable(client, method, arguments, name, predicate))

                for fixture in large_documents:
                    large_interactions(client, fixture, cycle, metrics)

                # Workspace search/references run alongside the foreground request.
                began = time.monotonic()
                reference = client.send("textDocument/references", {**symbol_params, "context": {"includeDeclaration": True}}, request=True)
                client.send("$/cancelRequest", {"id": reference})
                search_started = time.monotonic()
                search = client.send("workspace/symbol", {"query": symbol}, request=True)
                metrics["mixed_completion"].append(usable(client, "textDocument/completion", params, "mixed-completion", lambda r: has_symbol(r, symbol)))
                cancelled = client.wait_response(reference, "textDocument/references", "cancel", began, timeout=30)
                if cancelled.get("error", {}).get("code") == -32800:
                    # Use reader receipt time, not time spent draining other replies.
                    metrics["cancel_response"].append(client.samples[-1]["latency_ms"])
                elif cancelled.get("error", {}).get("code") not in (None, -32801, -32802):
                    raise AssertionError(cancelled)
                searched = client.wait_response(search, "workspace/symbol", "background-search", search_started, timeout=30)
                if "error" in searched:
                    raise AssertionError(searched)

                broken = source.replace("__sail_perf_identity(v)\n", "true\n")
                change(broken)
                usable(client, "textDocument/diagnostic", {"textDocument": document}, "broken-diagnostics", lambda r: target_error(r, True))
                began = time.monotonic()
                change(edited)
                usable(client, "textDocument/diagnostic", {"textDocument": document}, "repaired-diagnostics", lambda r: target_error(r, False))
                metrics["diagnostic_settle"].append((time.monotonic() - began) * 1000)

                if (cycle + 1) % policy["reloadEvery"] == 0:
                    event_start = len(client.events)
                    client.send("sail-lsp/reloadWorkspace", None)
                    deadline = time.monotonic() + 60
                    while not any(e.get("method") == "$/progress" and e.get("params", {}).get("value", {}).get("kind") == "begin"
                                  and e["params"]["value"].get("title") == "Reloading workspace" for e in client.events[event_start:]):
                        if time.monotonic() >= deadline:
                            raise TimeoutError("reload did not start")
                        client.pump(.01)
                    metrics["reload_completion"].append(usable(client, "textDocument/completion", params, "during-reload", lambda r: has_symbol(r, symbol)))
                    wait_ready(client, event_start)
            if len(metrics["cancel_response"]) < policy["minimumCancelledResponses"]:
                raise AssertionError("Inconclusive fixture: insufficient actual RequestCancelled replies")
            client.ask("shutdown", None, "shutdown")
            client.send("exit", None)
            client.process.wait(timeout=15)
            if client.process.returncode:
                raise AssertionError(f"Server exited {client.process.returncode}")
        finally:
            if client.process.poll() is None:
                client.process.kill()
                client.process.wait()
            client.reader.join(timeout=2)
            stderr.flush()
            metrics["interactive_queue"] = queue_samples(log_path.read_text())
            output.write_text(json.dumps({"metrics": metrics, "samples": client.samples, "events": client.events}, indent=2) + "\n")
    return metrics


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--server", type=Path, required=True)
    p.add_argument("--riscv", type=Path, required=True)
    p.add_argument("--arm", type=Path, required=True)
    p.add_argument("--sail-src", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--cpu-affinity")
    p.add_argument("--policy", type=Path, default=ROOT / "benchmarks/real-model-performance.json")
    args = p.parse_args()
    policy = json.loads(args.policy.read_text())
    validate_policy(policy)
    args.output.mkdir(parents=True, exist_ok=True)
    server = args.server.resolve(strict=True)
    # The explicit Sail source pin determines the standard library as well.
    sail_info = check_pin(args.sail_src.resolve(), "ac09f8870b9d85e528e2a3436150b03b9d805d40")
    os.environ["SAIL_DIR"] = str(args.sail_src.resolve())
    os.environ["SAIL_LOG"] = "sail_lsp::latency=debug"
    report = {"status": "running", "policy": policy, "environment": fingerprint(args.cpu_affinity),
              "serverSha256": digest(server), "build": version(server), "sail": sail_info,
              "lspSource": provenance(ROOT, 30), "sourceSha256": source_fingerprint(),
              "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
              "scriptSha256": digest(Path(__file__)), "models": {}}
    try:
        with tempfile.TemporaryDirectory(prefix="sail-real-model-") as temporary:
            scratch = Path(temporary)
            for name, path in [("riscv", args.riscv.resolve()), ("arm", args.arm.resolve())]:
                info = check_pin(path, policy["models"][name]["revision"])
                if name == "arm":
                    manifest, adapter = arm_adapter(path / "arm-v9.4-a", scratch / "arm", "devices.sail")
                    root = manifest.parent
                    info["adapter"] = adapter
                else:
                    root, manifest = path, path / "model/riscv.sail_project"
                entry = {"provenance": info, "runs": []}
                report["models"][name] = entry
                for repeat in range(policy["repeats"]):
                    entry["runs"].append(measure(server, root, manifest, policy["models"][name], policy,
                                                 args.output / f"{name}-{repeat}.json", args.cpu_affinity))
                entry["checks"] = evaluate(entry["runs"], policy)
        if report["sourceSha256"] != source_fingerprint() or report["serverSha256"] != digest(server):
            raise ValueError("Build inputs or binary changed during the experiment")
        report["status"] = "passed" if all(c["passed"] for m in report["models"].values() for c in m["checks"]) else "failed"
    except Exception as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}")
    finally:
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "error": report.get("error"), "report": str(args.output / "report.json")}))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
