#!/usr/bin/env python3
"""Measure a real server over stdio without changing workspace files.

Example: python3 scripts/editor_probe.py --server target/release/sail-lsp \
  --workspace ../sail-riscv --output target/editor-probe.json
Latency includes transport, queueing and analysis. No timing assertions are
made: compare repeated runs on the same host. RSS is Linux process RSS, not
live allocation size. Each response and diagnostic publication is retained.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


class Client:
    def __init__(self, server, stderr):
        self.process = subprocess.Popen(
            [str(server)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr
        )
        self.incoming = queue.Queue()
        self.next_id = 0
        self.responses = {}
        self.events = []
        self.samples = []
        self.priming = set()
        self.prime_completions = 0
        self.prime_failures = 0
        self.started = time.monotonic()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        raise EOFError("server stdout closed")
                    if line in (b"\r\n", b"\n"):
                        break
                    name, value = line.decode("ascii").split(":", 1)
                    headers[name.lower()] = value.strip()
                size = int(headers["content-length"])
                body = self.process.stdout.read(size)
                if len(body) != size:
                    raise EOFError("truncated LSP message")
                self.incoming.put(json.loads(body))
        except Exception as error:
            self.incoming.put(error)

    def send(self, method, params, request=False):
        message = {"jsonrpc": "2.0", "method": method, "params": params}
        if request:
            self.next_id += 1
            message["id"] = self.next_id
        self.write(message)
        return message.get("id")

    def write(self, message):
        body = json.dumps(message).encode("utf-8")
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.process.stdin.flush()

    def pump(self, timeout):
        try:
            message = self.incoming.get(timeout=max(0, timeout))
        except queue.Empty:
            return
        if isinstance(message, Exception):
            raise message
        if "method" in message:
            self.events.append({"elapsed_ms": self.elapsed(), **message})
            if message["method"] == "$/progress":
                params = message["params"]
                token, value = params["token"], params["value"]
                if value.get("kind") == "begin" and value.get("title") == "Priming caches":
                    self.priming.add(token)
                elif value.get("kind") == "end" and token in self.priming:
                    self.priming.remove(token)
                    if value.get("message") == "done":
                        self.prime_completions += 1
                    elif value.get("message") == "failed":
                        self.prime_failures += 1
            if "id" in message:
                # Registration/progress requests are acknowledged, not ignored.
                result = None
                if message["method"] == "workspace/configuration":
                    result = [None for _ in message["params"]["items"]]
                self.write({"jsonrpc": "2.0", "id": message["id"], "result": result})
        else:
            self.responses[message["id"]] = message

    def elapsed(self):
        return round((time.monotonic() - self.started) * 1000, 3)

    def ask(self, method, params, phase, timeout=180):
        started = time.monotonic()
        request_id = self.send(method, params, request=True)
        return self.wait_response(request_id, method, phase, started, timeout)

    def wait_response(self, request_id, method, phase, started, timeout=180):
        deadline = started + timeout
        while request_id not in self.responses:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{phase}: {method} exceeded {timeout}s")
            self.pump(min(0.1, deadline - time.monotonic()))
        response = self.responses.pop(request_id)
        self.samples.append({
            "phase": phase, "method": method,
            "latency_ms": round((time.monotonic() - started) * 1000, 3),
            "response": response,
        })
        return response

    def drain(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(min(0.1, deadline - time.monotonic()))

    def wait_for_priming(self, completed_before):
        deadline = time.monotonic() + 300
        while self.prime_completions <= completed_before or self.priming:
            if self.prime_failures:
                raise RuntimeError("server reported cache priming failure")
            if time.monotonic() >= deadline:
                raise TimeoutError("cache priming did not finish within 300s")
            self.pump(0.1)
        # Allow the completion event's cleanup/status messages to reach us.
        self.drain(0.2)

    def memory(self, phase, include_heap=False):
        result = {"phase": phase, "elapsed_ms": self.elapsed()}
        try:
            for line in Path(f"/proc/{self.process.pid}/status").read_text().splitlines():
                if line.startswith(("VmRSS:", "VmHWM:", "Threads:")):
                    key, value = line.split(":", 1)
                    result[key] = value.strip()
        except FileNotFoundError:
            pass
        if include_heap:
            result["analyzer_status"] = self.ask("sail-lsp/analyzerStatus", None, "resource-status")
        return result


def summary(samples):
    grouped = {}
    for sample in samples:
        key = sample["phase"] + ":" + sample["method"]
        grouped.setdefault(key, []).append(sample)
    result = {}
    for key, group in grouped.items():
        values = sorted(sample["latency_ms"] for sample in group)
        result[key] = {
            "count": len(values),
            "p50_ms": values[math.ceil(len(values) * 0.5) - 1],
            "p95_ms": values[math.ceil(len(values) * 0.95) - 1],
            "p99_ms": values[math.ceil(len(values) * 0.99) - 1],
            "max_ms": values[-1],
            "errors": [sample["response"]["error"] for sample in group
                       if "error" in sample["response"]],
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--file", default="model/core/types.sail")
    parser.add_argument("--symbol", default="creg2reg_idx")
    parser.add_argument("--completion-prefix", default="zero_ext")
    parser.add_argument("--cycles", type=int, default=30)
    parser.add_argument("--cancel-every", type=int, default=0)
    parser.add_argument("--lifecycle-cycles", type=int, default=0)
    parser.add_argument("--settle", action="store_true", help="wait for actual cache-prime completion before warm/reload samples")
    parser.add_argument("--heap-status", action="store_true", help="record allocator estimate from analyzerStatus at settled checkpoints")
    parser.add_argument("--idle-seconds", type=float, default=10)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.workspace.resolve()
    source_path = root / args.file
    source = source_path.read_text()
    offset = source.index(args.symbol)
    prefix = source[:offset]
    position = {"line": prefix.count("\n"),
                "character": len(prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2}
    completion_offset = source.index(args.completion_prefix) + len(args.completion_prefix)
    completion_prefix = source[:completion_offset]
    completion_position = {
        "line": completion_prefix.count("\n"),
        "character": len(completion_prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2,
    }
    uri = source_path.as_uri()
    document = {"uri": uri}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = {"workspace": str(root), "file": args.file, "cycles": args.cycles,
              "server": str(args.server.resolve()), "memory": [],
              "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "settle": args.settle, "lifecycle_cycles": args.lifecycle_cycles,
              "cancel_every": args.cancel_every, "idle_seconds": args.idle_seconds,
              "sail_dir": os.environ.get("SAIL_DIR"),
              "scope": "stdio, unsaved edits only; cold means fresh process, not cold OS cache"}
    with args.output.with_suffix(".stderr.log").open("w") as stderr:
        client = Client(args.server.resolve(), stderr)
        try:
            client.ask("initialize", {
                "processId": os.getpid(), "rootUri": root.as_uri(),
                "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
                "capabilities": {
                    "window": {"workDoneProgress": True},
                    "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": True}},
                    "textDocument": {"completion": {"completionItem": {"snippetSupport": True}}},
                },
            }, "startup")
            client.send("initialized", {})
            client.send("textDocument/didOpen", {"textDocument": {
                "uri": uri, "languageId": "sail", "version": 1, "text": source,
            }})
            methods = ("textDocument/hover", "textDocument/completion", "textDocument/definition")
            for method in methods:
                at = completion_position if method.endswith("/completion") else position
                client.ask(method, {"textDocument": document, "position": at}, "cold")
            deadline = time.monotonic() + 180
            while not any("ready" in event.get("params", {}).get("message", "")
                          for event in client.events):
                if time.monotonic() > deadline:
                    raise TimeoutError("workspace did not become ready")
                client.pump(0.1)
            report["ready_ms"] = next(event["elapsed_ms"] for event in client.events
                if "ready" in event.get("params", {}).get("message", ""))
            report["memory"].append(client.memory("ready"))
            if args.settle:
                client.wait_for_priming(0)
                report["memory"].append(client.memory("initial-prime-complete", args.heap_status))
            for _ in range(10):
                for method in methods:
                    at = completion_position if method.endswith("/completion") else position
                    response = client.ask(method, {"textDocument": document, "position": at}, "warm")
                    if "error" not in response and not response.get("result"):
                        raise AssertionError(f"{method} returned no usable result after loading")
            for cycle in range(args.cycles):
                # Add/remove a character in a comment, through incremental edits.
                # This measures revision churn without changing model semantics.
                client.send("textDocument/didChange", {
                    "textDocument": {"uri": uri, "version": cycle + 2},
                    "contentChanges": [{"range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0} if cycle % 2 == 0
                        else {"line": 1, "character": 0},
                    }, "text": "// editor probe\n" if cycle % 2 == 0 else ""}],
                })
                for method in methods:
                    at = completion_position if method.endswith("/completion") else position
                    current_position = {**at, "line": at["line"] + (cycle % 2 == 0)}
                    client.ask(method, {"textDocument": document, "position": current_position}, "editing")
                report["memory"].append(client.memory(f"edit-{cycle}"))
                if args.cancel_every and cycle % args.cancel_every == 0:
                    started = time.monotonic()
                    method = "textDocument/references"
                    request_id = client.send(method, {
                        "textDocument": document,
                        "position": {**position, "line": position["line"] + (cycle % 2 == 0)},
                        "context": {"includeDeclaration": True},
                    }, request=True)
                    client.send("$/cancelRequest", {"id": request_id})
                    client.wait_response(request_id, method, "client-cancel", started)
            client.send("textDocument/didClose", {"textDocument": document})
            client.drain(3)
            report["memory"].append(client.memory("closed"))
            for cycle in range(args.lifecycle_cycles):
                # Reopen with disk contents, then force the normal reload path.
                # No files or project configuration are written by this probe.
                client.send("textDocument/didOpen", {"textDocument": {
                    "uri": uri, "languageId": "sail", "version": 1, "text": source,
                }})
                event_start = len(client.events)
                primes_before = client.prime_completions
                client.send("sail-lsp/reloadWorkspace", None)
                deadline = time.monotonic() + 180
                while not any("ready" in event.get("params", {}).get("message", "")
                              for event in client.events[event_start:]):
                    if time.monotonic() > deadline:
                        raise TimeoutError("reload did not finish")
                    client.pump(0.1)
                if args.settle:
                    client.wait_for_priming(primes_before)
                for method in methods:
                    at = completion_position if method.endswith("/completion") else position
                    client.ask(method, {"textDocument": document, "position": at}, "reopened")
                client.send("textDocument/didClose", {"textDocument": document})
                client.drain(0.1)
                report["memory"].append(client.memory(f"reload-close-{cycle}", args.heap_status))
                print(json.dumps({"checkpoint": report["memory"][-1]}), flush=True)
            if args.lifecycle_cycles:
                client.drain(args.idle_seconds)
                report["memory"].append(client.memory("idle-after-lifecycle", args.heap_status))
            client.ask("shutdown", None, "shutdown")
            client.send("exit", None)
            client.process.wait(timeout=15)
            report["exit_code"] = client.process.returncode
        except Exception as error:
            report["failure"] = repr(error)
            raise
        finally:
            if client.process.poll() is None:
                client.process.kill()
                client.process.wait()
            client.reader.join(timeout=2)
            report["samples"] = client.samples
            report["summary"] = summary(client.samples)
            report["events"] = client.events
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({"output": str(args.output), "summary": report["summary"]}, indent=2))


if __name__ == "__main__":
    main()
