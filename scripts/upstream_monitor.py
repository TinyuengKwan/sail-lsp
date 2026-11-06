#!/usr/bin/env python3
"""Compare project evaluation with a real, unmodified Sail compiler.

The JSON report separates semantic differences, unsupported CLI inputs and
infrastructure failures. A timeout or failed graph command can never agree
with an empty graph. No server/compiler patch is applied by this program.
"""

import argparse
import collections
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time


class InfrastructureError(RuntimeError):
    pass


class UnsupportedConfiguration(ValueError):
    pass


def command(argv, cwd, timeout, input_text=None):
    started = time.monotonic()
    try:
        result = subprocess.run(
            [str(a) for a in argv], cwd=cwd, input=input_text,
            capture_output=True, text=True, encoding="utf-8", errors="strict",
            timeout=timeout,
        )
    except (OSError, UnicodeError, subprocess.TimeoutExpired) as error:
        raise InfrastructureError(f"{argv}: {error}") from error
    return {
        "argv": [str(a) for a in argv], "cwd": str(cwd),
        "exit_code": result.returncode, "stdout": result.stdout,
        "stderr": result.stderr, "elapsed_seconds": time.monotonic() - started,
    }


def checked(argv, cwd, timeout, input_text=None):
    result = command(argv, cwd, timeout, input_text)
    if result["exit_code"]:
        raise InfrastructureError(json.dumps(result, ensure_ascii=False))
    return result


def provenance(path, timeout):
    path = Path(path).resolve(strict=True)
    result = {"path": str(path)}
    if path.is_file():
        with path.open("rb") as source:
            result["sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
    repository = path if path.is_dir() else path.parent
    git = command(["git", "-C", repository, "rev-parse", "HEAD"], None, timeout)
    if not git["exit_code"]:
        result["revision"] = git["stdout"].strip()
        status = checked(["git", "-C", repository, "status", "--porcelain=v1", "--untracked-files=no"], None, timeout)
        result["tracked_changes"] = status["stdout"]
    return result


def variable_args(variables):
    args = []
    for name, value in sorted(variables.items()):
        if not name or name.strip() != name or "=" in name:
            raise UnsupportedConfiguration(f"variable name cannot round-trip through Sail CLI: {name!r}")
        if isinstance(value, bool):
            value = str(value).lower()
        elif isinstance(value, str):
            if value.strip() != value or value in ("true", "false"):
                raise UnsupportedConfiguration(f"typed string cannot round-trip through Sail CLI: {name}={value!r}")
        else:
            raise UnsupportedConfiguration(f"unsupported CLI variable: {name}={value!r}")
        args.extend(["--variable", f"{name}={value}"])
    return args


def parse_graph(text):
    if not text.strip().startswith("digraph ") or not text.strip().endswith("}"):
        raise InfrastructureError(f"compiler did not produce a DOT graph: {text!r}")
    quoted = r'"(?:[^"\\]|\\.)*"'
    graph, nodes = {}, set()
    for line in text.splitlines():
        names = re.findall(quoted, line)
        if "->" in line:
            if len(names) != 2:
                raise InfrastructureError(f"unrecognized DOT edge: {line!r}")
            required, by = (json.loads(n) for n in names)
            if by != "corelib" and required != "corelib":
                graph.setdefault(by, set()).add(required)
        elif names:
            if len(names) != 1:
                raise InfrastructureError(f"unrecognized DOT node: {line!r}")
            name = json.loads(names[0])
            if name != "corelib":
                nodes.add(name)
    return {name: sorted(edges) for name, edges in sorted(graph.items())}, sorted(nodes)


class Monitor:
    def __init__(self, compiler, oracle, timeout, report):
        self.compiler, self.oracle, self.timeout, self.report = compiler, oracle, timeout, report

    def lsp(self, text, variables, roots, cwd):
        result = checked(
            [self.oracle], cwd, self.timeout,
            json.dumps({"manifest": text, "variables": variables, "roots": roots}) + "\n",
        )
        try:
            answer = json.loads(result["stdout"])
            if not isinstance(answer.get("accepted"), bool):
                raise ValueError("missing accepted boolean")
            if answer["accepted"]:
                for name in ("files", "modules"):
                    values = answer.get(name)
                    if not isinstance(values, list) or not all(isinstance(value, str) for value in values):
                        raise ValueError(f"invalid {name}")
                requires = answer.get("requires")
                if not isinstance(requires, dict) or not all(
                    isinstance(edges, list) and all(isinstance(edge, str) for edge in edges)
                    for edges in requires.values()
                ):
                    raise ValueError("invalid requires graph")
            elif not isinstance(answer.get("error"), str):
                raise ValueError("missing rejection error")
        except (ValueError, AttributeError) as error:
            raise InfrastructureError(f"invalid LSP oracle result: {result}") from error
        return answer

    def compare(self, label, manifest, variables, adapter=None, expect_accepted=False):
        row = {"id": label, "manifest": str(manifest), "variables": variables, "adapter": adapter, "checks": []}
        self.report["cases"].append(row)
        try:
            text = manifest.read_text()
            row["manifest_sha256"] = hashlib.sha256(text.encode()).hexdigest()
            row["manifest_source"] = text
            cli = variable_args(variables)
            all_modules = self.lsp(text, variables, [], manifest.parent)
            for source in all_modules.get("files", []):
                if not (manifest.parent / source).is_file():
                    raise InfrastructureError(f"project source input does not exist: {manifest.parent / source}")
            # Exercise each root and a multi-root union in addition to all modules.
            modules = all_modules.get("modules", [])
            selections = [[]] + [[m] for m in modules]
            if len(modules) > 1:
                selections.append([modules[0], modules[-1]])
            for roots in selections:
                ours = all_modules if not roots else self.lsp(text, variables, roots, manifest.parent)
                upstream = command(
                    [self.compiler, "--project", manifest.name, "--list-files-separated", "\x1e", *cli,
                     *(roots or ["--all-modules"])], manifest.parent, self.timeout,
                )
                if upstream["exit_code"] < 0:
                    raise InfrastructureError(f"compiler terminated by signal: {upstream}")
                theirs = upstream["exit_code"] == 0
                check = {"kind": "files", "roots": roots, "lsp": ours, "upstream": upstream}
                row["checks"].append(check)
                if ours["accepted"] != theirs or (expect_accepted and not theirs):
                    check["classification"] = "acceptance_difference"
                    plain_error = re.sub(r"\x1b\[[0-9;]*m", "", upstream["stderr"])
                    if ours["accepted"] and not theirs and "Syntax error" in plain_error and re.search(r"\.sail:\d", plain_error):
                        check["classification"] = "source_syntax_rejection"
                elif not theirs:
                    # Both reject: preserve both errors without claiming their
                    # causes are equivalent. Accepted files get the full checks.
                    check["classification"] = "both_rejected"
                else:
                    stdout = upstream["stdout"]
                    if not stdout.endswith("\n"):
                        raise InfrastructureError("file-list output is not newline terminated")
                    stdout = stdout[:-1]
                    files = stdout.split("\x1e") if stdout else []
                    check["classification"] = "agreement" if files == ours["files"] else "file_order_difference"
                    check["upstream_files"] = files
            if all_modules["accepted"]:
                graph = checked(
                    [self.compiler, "--project", manifest.name, *cli, "--all-modules", "--just-parse-project",
                     "--ddump-project-depgraph", "stdout", "--ddump-project-depgraph-skip-deps"],
                    manifest.parent, self.timeout,
                )
                requires, nodes = parse_graph(graph["stdout"])
                agrees = requires == all_modules["requires"] and nodes == sorted(modules)
                row["checks"].append({
                    "kind": "requires", "classification": "agreement" if agrees else "dependency_difference",
                    "lsp": {"requires": all_modules["requires"], "nodes": sorted(modules)},
                    "upstream": graph, "upstream_graph": {"requires": requires, "nodes": nodes},
                })
            if not row["checks"]:
                raise InfrastructureError("case produced no comparisons")
        except UnsupportedConfiguration as error:
            row["checks"].append({"kind": "configuration", "classification": "unsupported", "error": str(error)})
        except (InfrastructureError, OSError, ValueError) as error:
            row["checks"].append({"kind": "execution", "classification": "infrastructure_failure", "error": str(error)})


def arm_adapter(model, scratch, devices):
    """Read the ordered source list, never infer it from directory enumeration.

    sail-arm has no native project manifest. This deliberately tests a wrapper
    of the documented Makefile input list; it does not assert typecheck parity.
    Fail on a changed Makefile expression instead of silently dropping sources.
    """
    makefile = model / "Makefile"
    text = makefile.read_text()
    match = re.search(r"^SAIL_SRCS\s*=([^\n]+)$", text, re.M)
    if not match:
        raise InfrastructureError(f"no supported SAIL_SRCS assignment in {makefile}")
    sources = match[1].replace("$(DEVICES_PATH)", f"src/{devices}").split()
    if not sources or any("$" in source for source in sources):
        raise InfrastructureError(f"unsupported ordered source expression in {makefile}")
    for source in sources:
        if not (model / source).is_file():
            raise InfrastructureError(f"Makefile source does not exist: {model / source}")
    scratch.mkdir()
    (scratch / "src").symlink_to(model / "src", target_is_directory=True)
    manifest = scratch / "arm.sail_project"
    manifest.write_text("Arm { files " + ", ".join(json.dumps(p) for p in sources) + " }\n")
    return manifest, {
        "kind": "ordered_makefile_sources", "makefile": str(makefile),
        "makefile_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "sources": sources, "devices": devices,
    }


def run(args, report):
    compiler = args.compiler.resolve(strict=True)
    oracle = args.lsp_oracle.resolve(strict=True)
    report["inputs"] = {
        "compiler": provenance(compiler, args.timeout), "lsp_oracle": provenance(oracle, args.timeout),
        "sail": provenance(args.sail_src, args.timeout), "riscv": provenance(args.riscv, args.timeout),
        "lsp_source": provenance(Path(__file__).resolve().parent.parent, args.timeout),
    }
    with Path(__file__).open("rb") as source:
        report["inputs"]["monitor_sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
    version = checked([compiler, "--version"], None, args.timeout)["stdout"].strip()
    if not version:
        raise InfrastructureError("empty compiler version")
    report["compiler_version"] = version
    monitor = Monitor(compiler, oracle, args.timeout, report)
    for directory in ("test/typecheck/project", "test/project/failure"):
        manifests = sorted((args.sail_src.resolve(strict=True) / directory).glob("*.sail_project"))
        if not manifests:
            raise InfrastructureError(f"no manifests under {directory}")
        for path in manifests:
            monitor.compare(f"sail/{directory}/{path.name}", path, {})
    riscv = args.riscv.resolve(strict=True) / "model/riscv.sail_project"
    for rmem in (False, True):
        monitor.compare(f"riscv/RMEM={rmem}", riscv, {"RMEM": rmem}, expect_accepted=True)
    with tempfile.TemporaryDirectory(prefix="sail-upstream-monitor-") as temp:
        scratch = Path(temp)
        # Variable-sensitive dependencies expose an oracle that only passes
        # variables to --list-files; the spaced path exposes whitespace splitting.
        manifest = scratch / "conditional.sail_project"
        manifest.write_text(
            'variable PICK = false\nA { files "a space.sail" }\nC { files c.sail }\n'
            'B { requires if $PICK then A else C files b.sail }\n'
        )
        for name in ("a space.sail", "b.sail", "c.sail"):
            (scratch / name).write_text("")
        for pick in (False, True):
            monitor.compare(f"conditional/PICK={pick}", manifest, {"PICK": pick}, expect_accepted=True)
        if args.arm_src:
            arm = args.arm_src.resolve(strict=True)
            report["inputs"]["arm"] = provenance(arm, args.timeout)
            variants = []
            for model in args.arm_model or ["arm-v9.4-a"]:
                variants.append((model, "devices.sail"))
                if model == "arm-v9.4-a":
                    variants.append((model, "no_devices.sail"))
            for model, device in variants:
                label = f"arm/{model}/{device}"
                # A changed/absent model is an explicit failed case, not skipped.
                try:
                    path, adapter = arm_adapter(arm / model, scratch / f"{model}-{device}", device)
                except (InfrastructureError, OSError) as error:
                    report["cases"].append({"id": label, "checks": [{"classification": "infrastructure_failure", "error": str(error)}]})
                else:
                    monitor.compare(label, path, {}, adapter, expect_accepted=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--lsp-oracle", type=Path, required=True)
    parser.add_argument("--sail-src", type=Path, required=True)
    parser.add_argument("--riscv", type=Path, required=True)
    parser.add_argument("--arm-src", type=Path)
    parser.add_argument("--arm-model", action="append", choices=("arm-v9.3-a", "arm-v9.4-a"), help="repeat to compare multiple model versions; default arm-v9.4-a (9.3 sources use the now-reserved `from` identifier)")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {"schema_version": 1, "started_at_unix": time.time(), "scope": "project file occurrence order, root selection and direct requires; not body typecheck parity", "cases": []}
    try:
        if args.timeout <= 0:
            raise InfrastructureError("timeout must be positive")
        run(args, report)
    except (InfrastructureError, OSError) as error:
        report["setup_failure"] = str(error)
    counts = collections.Counter(check["classification"] for case in report["cases"] for check in case["checks"])
    report["summary"] = dict(sorted(counts.items()))
    report["finished_at_unix"] = time.time()
    report["passed"] = bool(report["cases"]) and not report.get("setup_failure") and all(name in {"agreement", "both_rejected"} for name in counts)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({"report": str(args.output), "passed": report["passed"], "summary": report["summary"], "setup_failure": report.get("setup_failure")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
