#!/usr/bin/env python3
"""Replay unfinished Sail edits over stdio, retaining responses and diagnostics.

Uses a temporary project; never writes to the user's model. Missing expected
completions, navigation, error recovery, or duplicate responses fail the run.
Timing observations have no single-run pass/fail threshold.
"""
import argparse
from collections import Counter
import hashlib
import json
import tempfile
import time
from pathlib import Path

from editor_probe import Client, summary


BASE = ("val probe_identity : int -> int\n"
        "function probe_identity(value) = value\n"
        "val stable : int -> int\n"
        "function stable(value) = probe_identity(value)\n")
SIGNATURE = "val edit_me : int -> int\n"
CASES = [
    ("partial-name", "function edit_me(input) = probe_ide$0", "probe_identity", None),
    ("open-call", "function edit_me(input) = probe_identity($0", "input", None),
    ("partial-argument", "function edit_me(input) = probe_identity(inp$0", "input", None),
    ("closed-call", "function edit_me(input) = probe_identity(input)$0\n", None, False),
    ("partial-match", "function edit_me(input) = match input { 0 => probe_ide$0", "probe_identity", None),
    ("partial-if", "function edit_me(input) = if true then probe_ide$0", "probe_identity", None),
    ("open-block", "function edit_me(input) = { let tmp = input; probe_ide$0", "probe_identity", None),
    ("type-error", "function edit_me(input) = probe_identity(true)$0\n", None, True),
    ("repaired", "function edit_me(input) = probe_identity(input)$0\n", None, False),
    ("existential-type", "type Box = { 'n : Int, 'n >= 0. bi$0", "bits", None),
    ("missing-include", '$include "absent.sail"\nfunction edit_me(input) = provided(input)$0\n', None, True),
    ("missing-include-independent-error", '$include "absent.sail"\nfunction edit_me(input) = probe_identity(true)$0\n', None, True),
    ("broken-signature", "val broken : (int, $0", None, True),
    ("repaired-again", "function edit_me(input) = probe_identity(input)$0\n", None, False),
]


def position(text, offset):
    prefix = text[:offset]
    return {"line": prefix.count("\n"),
            "character": len(prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2}


def ask(client, method, params, phase):
    for attempt in range(6):
        response = client.ask(method, params, phase if not attempt else phase + ":retry")
        if response.get("error", {}).get("code") not in (-32801, -32802):
            if "error" in response:
                raise AssertionError(response)
            return response.get("result")
        client.drain(.05)
    raise AssertionError(f"repeated cancellation: {phase} {method}")


def run(server, output):
    output.parent.mkdir(parents=True, exist_ok=True)
    report = {"server_sha256": hashlib.sha256(server.read_bytes()).hexdigest(),
              "scope": "temporary one-module project; full unsaved document versions",
              "cases": [], "failures": []}
    with tempfile.TemporaryDirectory(prefix="sail-editing-replay-") as temp:
        root = Path(temp)
        initial = BASE + SIGNATURE + CASES[3][1].replace("$0", "")
        (root / "main.sail").write_text(initial)
        (root / "sail.proj").write_text("Main { files main.sail }\n")
        document = {"uri": (root / "main.sail").as_uri()}
        with output.with_suffix(".stderr.log").open("w") as stderr:
            client = Client(server, stderr)
            try:
                client.ask("initialize", {
                    "rootUri": root.as_uri(),
                    "workspaceFolders": [{"uri": root.as_uri(), "name": "editing-replay"}],
                    "capabilities": {"window": {"workDoneProgress": True},
                                     "textDocument": {"diagnostic": {}}},
                }, "startup")
                client.send("initialized", {})
                client.send("textDocument/didOpen", {"textDocument": {
                    **document, "languageId": "sail", "version": 1, "text": initial}})
                deadline = time.monotonic() + 180
                while not any("ready" in e.get("params", {}).get("message", "")
                              for e in client.events):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("project did not load")
                    client.pump(.1)
                for version, (name, fragment, expected, error_expected) in enumerate(CASES, 2):
                    marked = BASE + ("" if name == "existential-type" else SIGNATURE) + fragment
                    cursor = marked.index("$0")
                    source = marked.replace("$0", "")
                    record = {"name": name, "version": version, "source": source,
                              "cursor": position(source, cursor)}
                    report["cases"].append(record)
                    edit_started = time.monotonic()
                    client.send("textDocument/didChange", {
                        "textDocument": {**document, "version": version},
                        "contentChanges": [{"text": source}]})
                    try:
                        if expected:
                            result = ask(client, "textDocument/completion", {
                                "textDocument": document, "position": record["cursor"]}, name)
                            items = result.get("items", []) if isinstance(result, dict) else result or []
                            record["completion_labels"] = [item["label"] for item in items]
                            assert expected in record["completion_labels"], (name, expected, record["completion_labels"])
                        # Verify the intact reference and, when present, the
                        # callee in the unfinished expression as well.
                        offsets = [source.index("probe_identity(value)", source.index("function stable"))]
                        live = source.find("probe_identity(", len(BASE))
                        if live >= 0:
                            offsets.append(live)
                        for offset in offsets:
                            params = {"textDocument": document, "position": position(source, offset + 2)}
                            hover = ask(client, "textDocument/hover", params, name)
                            definition = ask(client, "textDocument/definition", params, name)
                            assert hover and definition, (name, "missing navigation", offset)
                            targets = definition if isinstance(definition, list) else [definition]
                            assert all(t.get("uri", t.get("targetUri")) == document["uri"] for t in targets)
                        diagnostic_started = time.monotonic()
                        diagnostic = ask(client, "textDocument/diagnostic", {"textDocument": document}, name)
                        record["diagnostics"] = diagnostic.get("items", [])
                        record["diagnostic_latency_ms"] = round((time.monotonic() - diagnostic_started) * 1000, 3)
                        record["edit_to_diagnostic_ms"] = round((time.monotonic() - edit_started) * 1000, 3)
                        record["diagnostic_count"] = len(record["diagnostics"])
                        record["diagnostic_codes"] = dict(Counter(d.get("code") for d in record["diagnostics"]))
                        # Keep independent semantic findings visible. These are
                        # categories, not inferred causal links across files.
                        record["root_diagnostics"] = [d for d in record["diagnostics"] if d.get("code") in {"syntax-error", "unresolved-include"}]
                        record["semantic_diagnostics"] = [d for d in record["diagnostics"] if d not in record["root_diagnostics"]]
                        maxima = {"open-call": 1, "partial-argument": 2, "partial-match": 2, "open-block": 3}
                        if name in maxima:
                            assert record["diagnostic_count"] <= maxima[name], (name, "editing noise", record["diagnostics"])
                        if name.startswith("missing-include"):
                            assert record["diagnostic_codes"].get("unresolved-include") == 1, record
                        if name == "missing-include-independent-error":
                            assert record["diagnostic_codes"].get("type-error"), "independent type error was hidden"
                        errors = [d for d in record["diagnostics"] if d.get("severity") == 1]
                        if error_expected is not None:
                            assert bool(errors) == error_expected, (name, "error recovery", errors)
                    except AssertionError as error:
                        report["failures"].append({"case": name, "error": repr(error)})
                client.ask("shutdown", None, "shutdown")
                client.send("exit", None)
                report["exit_code"] = client.process.wait(timeout=15)
            except Exception as error:
                report["failures"].append({"case": "protocol", "error": repr(error)})
            finally:
                if client.process.poll() is None:
                    client.process.kill()
                    client.process.wait()
                client.reader.join(timeout=2)
                report["samples"] = client.samples
                report["events"] = client.events
                report["summary"] = summary(client.samples)
                output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(output), "cases": len(report["cases"]),
                      "failures": report["failures"]}, indent=2))
    return bool(report["failures"]) or report.get("exit_code") != 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(run(args.server.resolve(), args.output))
