#!/usr/bin/env python3
"""Exercise resource recovery through stdio in an isolated, generated project.

The project has many physical files, shared include views and concurrently open
edited buffers. No user workspace is modified. Checkpoint ownership counts are
assertions; allocator/RSS measurements are observations, not universal limits.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import time

from editor_probe import Client


def resource_counts(reply):
    text = reply.get("result")
    if not isinstance(text, str):
        raise AssertionError(f"analyzerStatus failed: {reply}")
    labels = {
        "physical_files": r"retained physical file inputs: (\d+)",
        "semantic_views": r"retained semantic view inputs: (\d+)",
        "text_bytes": r"unique retained text bytes: (\d+)",
        "parse_seeds": r"parse seeds: (\d+)",
        "snapshots": r"live analysis snapshots \(including this request\): (\d+)",
        "heap_mib": r"allocator heap:\s+(\d+) MiB",
    }
    counts = {}
    for name, pattern in labels.items():
        match = re.search(pattern, text)
        if not match:
            raise AssertionError(f"analyzerStatus omitted {name}")
        counts[name] = int(match[1])
    match = re.search(r"solver result cache entries: (?:Some\((\d+)\)|None) / (\d+)", text)
    if match:
        counts["solver_cache_entries"] = int(match[1]) if match[1] else None
        counts["solver_cache_capacity"] = int(match[2])
    match = re.search(r"solver threads: (\d+) live / (\d+) ever initialized", text)
    if match:
        counts["solver_threads_live"], counts["solver_threads_total"] = map(int, match.groups())
    return counts


def ask(client, method, params, phase):
    for attempt in range(6):
        response = client.ask(method, params, phase)
        error = response.get("error")
        if not error:
            return response.get("result")
        if error["code"] not in (-32801, -32802) or attempt == 5:
            raise AssertionError(f"{phase}: {response}")
        client.drain(0.05)
    raise AssertionError("unreachable retry exhaustion")


def source(index, functions, revision=0, broken=False):
    values = [f"function f_{index}_{j}() -> int = {revision if j == 0 else j}\n"
              for j in range(functions)]
    if broken:
        values[0] = f"function f_{index}_0() -> int = true\n"
    return '$include "shared.sail"\n' + ''.join(values)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--files", type=int, default=512)
    parser.add_argument("--functions", type=int, default=16)
    parser.add_argument("--open-files", type=int, default=32)
    parser.add_argument("--rounds", type=int, default=32)
    parser.add_argument("--duration-seconds", type=float, default=600)
    parser.add_argument("--reload-every", type=int, default=8)
    parser.add_argument("--watch-reloads", action="store_true",
                        help="rewrite the loaded manifest and report CREATED, as in atomic replacement")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not (args.files >= args.open_files > 0 and args.functions > 0 and args.rounds > 0
            and args.duration_seconds >= 0 and args.reload_every > 0):
        parser.error("positive file/function/round counts and nonnegative duration required")
    server = args.server.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = {"server": str(server), "server_sha256": hashlib.sha256(server.read_bytes()).hexdigest(),
              "files": args.files, "functions_per_file": args.functions,
              "concurrently_open_files": args.open_files, "minimum_rounds": args.rounds,
              "minimum_duration_seconds": args.duration_seconds, "watch_reloads": args.watch_reloads, "checkpoints": [],
              "scope": "isolated synthetic project; actual body edits and include views; not a real editor UI"}
    raw_path = args.output.with_suffix(".responses.jsonl")
    with tempfile.TemporaryDirectory(prefix="sail-resource-") as directory, \
            args.output.with_suffix(".stderr.log").open("w") as stderr, raw_path.open("w") as raw:
        root = Path(directory)
        (root / "toolchain" / "lib").mkdir(parents=True)
        (root / "shared.sail").write_text("// One shared input, many semantic include views.\ndefault Order dec\n")
        paths = [root / f"unit_{i:04}.sail" for i in range(args.files)]
        for i, path in enumerate(paths):
            path.write_text(source(i, args.functions))
        (root / "sail.proj").write_text("Main { files " + ", ".join(p.name for p in paths) + " }\n")
        previous_sail_dir = os.environ.get("SAIL_DIR")
        os.environ["SAIL_DIR"] = str(root / "toolchain")
        try:
            client = Client(server, stderr)
        finally:
            if previous_sail_dir is None:
                del os.environ["SAIL_DIR"]
            else:
                os.environ["SAIL_DIR"] = previous_sail_dir

        def flush():
            for sample in client.samples:
                raw.write(json.dumps({"kind": "response", **sample}) + "\n")
            for event in client.events:
                raw.write(json.dumps({"kind": "event", **event}) + "\n")
            raw.flush()
            client.samples.clear()
            client.events.clear()

        def checkpoint(phase):
            deadline = time.monotonic() + 30
            while True:
                client.drain(0.1)
                memory = client.memory(phase, include_heap=True)
                counts = resource_counts(memory["analyzer_status"])
                if counts["snapshots"] == 1:
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(f"snapshots did not settle: {counts}")
            memory["counts"] = counts
            report["checkpoints"].append(memory)
            print(json.dumps({"phase": phase, "elapsed_ms": memory["elapsed_ms"], **counts}), flush=True)
            if counts.get("solver_cache_entries") is not None:
                assert counts["solver_cache_entries"] <= counts["solver_cache_capacity"]
            # The main pool has at most eight persistent native context owners.
            assert counts.get("solver_threads_live", 0) <= 8
            flush()
            return counts

        try:
            ask(client, "initialize", {
                "processId": os.getpid(), "rootUri": root.as_uri(),
                "workspaceFolders": [{"uri": root.as_uri(), "name": "resource-replay"}],
                "capabilities": {"window": {"workDoneProgress": True}},
            }, "startup")
            client.send("initialized", {})
            client.wait_for_priming(0)
            baseline = checkpoint("initial")
            report["expected_semantic_views"] = args.files - 1
            assert baseline["semantic_views"] == args.files - 1, baseline
            began = time.monotonic()
            round_index = 0
            while round_index < args.rounds or time.monotonic() - began < args.duration_seconds:
                selected = [(round_index * args.open_files + j) % args.files for j in range(args.open_files)]
                for index in selected:
                    uri = paths[index].as_uri()
                    client.send("textDocument/didOpen", {"textDocument": {
                        "uri": uri, "languageId": "sail", "version": 1,
                        "text": source(index, args.functions)}})
                    client.send("textDocument/didChange", {
                        "textDocument": {"uri": uri, "version": 2},
                        "contentChanges": [{"text": source(index, args.functions, round_index + 1)}]})
                for index in selected:
                    document = {"textDocument": {"uri": paths[index].as_uri()}}
                    symbols = ask(client, "textDocument/documentSymbol", document, "edited-symbols")
                    assert symbols, document
                    diagnostics = ask(client, "textDocument/diagnostic", document, "edited-diagnostics")
                    assert not any(d.get("severity") == 1 for d in diagnostics["items"]), diagnostics
                if round_index % args.reload_every == 0:
                    # A genuine transient semantic error must appear, then clear
                    # after the next revision, including while other files are open.
                    index = selected[0]
                    document = {"textDocument": {"uri": paths[index].as_uri()}}
                    for version, broken in [(3, True), (4, False)]:
                        client.send("textDocument/didChange", {
                            "textDocument": {**document["textDocument"], "version": version},
                            "contentChanges": [{"text": source(index, args.functions, round_index + 1, broken)}]})
                        diagnostics = ask(client, "textDocument/diagnostic", document, "diagnostic-recovery")
                        errors = [d for d in diagnostics["items"] if d.get("severity") == 1]
                        assert bool(errors) == broken, diagnostics
                for index in selected:
                    client.send("textDocument/didClose", {"textDocument": {"uri": paths[index].as_uri()}})
                if (round_index + 1) % args.reload_every == 0:
                    primed = client.prime_completions
                    if args.watch_reloads:
                        manifest = root / "sail.proj"
                        replacement = root / "sail.proj.tmp"
                        replacement.write_text(manifest.read_text())
                        replacement.replace(manifest)
                        client.send("workspace/didChangeWatchedFiles", {"changes": [
                            {"uri": manifest.as_uri(), "type": 1}]})
                    else:
                        client.send("sail-lsp/reloadWorkspace", None)
                    client.wait_for_priming(primed)
                counts = checkpoint(f"closed-{round_index}")
                assert counts["physical_files"] == baseline["physical_files"], counts
                assert counts["semantic_views"] == baseline["semantic_views"], counts
                # Restored disk source and bounded active parse seeds must not
                # accumulate another copy of every overlay on every edit.
                assert counts["text_bytes"] <= baseline["text_bytes"] * 2, counts
                round_index += 1
            report["completed_rounds"] = round_index
            report["active_duration_seconds"] = round(time.monotonic() - began, 3)
            report["semantic_edits"] = round_index * args.open_files
            report["reloads"] = round_index // args.reload_every
            ask(client, "shutdown", None, "shutdown")
            client.send("exit", None)
            client.process.wait(timeout=15)
            report["exit_code"] = client.process.returncode
            assert report["exit_code"] == 0
        except Exception as error:
            report["failure"] = repr(error)
            raise
        finally:
            if client.process.poll() is None:
                client.process.kill()
                client.process.wait()
            client.reader.join(timeout=2)
            flush()
            report["responses"] = len(client.seen_response_ids)
            report["elapsed_seconds"] = round(time.monotonic() - client.started, 3)
            args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
