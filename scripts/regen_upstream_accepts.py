#!/usr/bin/env python3
"""Regenerate `docs/upstream-accepts.txt`, the oracle for every corpus
outside `sail/test/typecheck`.

Each file is classified three ways, because the first two are not enough to
read the false-negative column honestly:

  OK      upstream accepts the file on its own
  SCOPE   upstream rejects it on its own and accepts it with the bundled
          `sail-lib` in scope
  REJECT  upstream rejects it either way

The distinction exists because the two sides are not given the same program.
`conformance_check` hands sail-lsp the bundled `sail-lib` whether or not a
file `$include`s it, which is what an editor has. Scope in a real Sail
project does not come from `$include` at all: **sail-riscv's 158 model files
contain no `$include` directive between them**, and `riscv.sail_project`
lists the files per module with `requires` edges. So a file that names
`print_endline` without including `<prelude.sail>` is ordinary, and
upstream's per-file `--just-check` is not how that project is built. A
`SCOPE` file says nothing about the checker; counting one as a false
negative measures the harness.

Usage:
    python3 scripts/regen_upstream_accepts.py /path/to/sail/test [--sail sail]

Takes a few minutes; `sail` must be on PATH (or named with --sail).
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile

CORPORA = [
    "builtins",
    "c",
    "float",
    "lean",
    "mono",
    "ocaml",
    "pattern_completeness",
    "rocq",
    "smt",
]

HEADER = """\
# Which files upstream Sail itself accepts
#
# Built by `scripts/regen_upstream_accepts.py` with `sail --no-memo-z3
# --just-check` over every .sail file under
# sail/test/{{{corpora}}}.
#
# Three verdicts, not two:
#
#   OK      upstream accepts the file on its own.
#   SCOPE   upstream rejects it on its own and accepts it once the bundled
#           `sail-lib` is in scope. Says nothing about sail-lsp: the harness
#           hands every file that whole library, because that is what an
#           editor has. Scope in a real Sail project does not come from `$include`
#           at all — sail-riscv's 158 model files contain no `$include`
#           directive between them, and `riscv.sail_project` lists the files
#           per module with `requires` edges — so a file that names a prelude
#           function without including it is ordinary. Counting these as
#           false negatives measures the harness, not the checker.
#   REJECT  upstream rejects it either way. Only these are false negatives
#           when sail-lsp stays silent.
#
# A raw diagnostic count on these corpora is not a false-positive count
# either: only an `OK` file is one sail-lsp must be silent on.
#
# {ok} OK, {scope} SCOPE, {reject} REJECT.
#
# Regenerate when the sail checkout moves. Paths are absolute on purpose —
# they are the ones `corpus_check` reports against.
"""


def declared_order(text: str) -> str:
    m = re.search(r"^\s*default\s+Order\s+(dec|inc)\b", text, re.M)
    return m.group(1) if m else "dec"


def run(sail: str, args, cwd: str) -> bool:
    try:
        r = subprocess.run(
            [sail, "--no-memo-z3", "--just-check", *args],
            cwd=cwd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=120,
        )
        return r.returncode == 0
    except subprocess.TimeoutExpired:
        return False


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("test_root")
    ap.add_argument("--sail", default="sail")
    ap.add_argument(
        "--out",
        default=os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            "docs",
            "upstream-accepts.txt",
        ),
    )
    args = ap.parse_args()

    # A broken Sail installation must not turn every source into REJECT.
    with tempfile.TemporaryDirectory(prefix="sail-oracle-check-") as probe_dir:
        probe = os.path.join(probe_dir, "probe.sail")
        with open(probe, "w") as f:
            f.write('default Order dec\n$include <prelude.sail>\nfunction probe() -> int = 1\n')
        checked = subprocess.run([args.sail, "--no-memo-z3", "--just-check", probe], capture_output=True, text=True, timeout=30)
        if checked.returncode:
            print("Upstream oracle preflight failed; existing oracle was not changed:\n" + checked.stderr, file=sys.stderr)
            return 2

    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    lib_dir = os.path.join(repo, "crates", "sail-lsp", "data", "sail-lib")
    if not os.path.isdir(lib_dir):
        print(f"bundled sail-lib not found at {lib_dir}", file=sys.stderr)
        return 2

    # More than `prelude.sail`: the harness hands sail-lsp the whole bundled
    # library, and `prelude.sail` does not include all of it.
    # `generic_equality.sail` is the one that showed it — `SIGNALREG == Low`
    # on an enum has no overloading without it, so `test/ocaml/types.sail`
    # looked like a real rejection when it is the same scope difference as
    # the rest.
    #
    # But not the whole directory either, and this is *computed* rather than
    # assumed: the bundle is a copy that upstream 0.20.2 does not all accept
    # — `instr_kinds.sail` is a syntax error there — so passing every file
    # fails for every test and reports no scope difference at all. Each extra
    # is tried on top of the prelude once, and the ones that load are kept.
    lib_all = sorted(
        os.path.join(lib_dir, n) for n in os.listdir(lib_dir) if n.endswith(".sail")
    )
    prelude = os.path.join(lib_dir, "prelude.sail")
    if prelude not in lib_all:
        print(f"bundled prelude not found in {lib_dir}", file=sys.stderr)
        return 2

    rows = []
    counts = {"OK": 0, "SCOPE": 0, "REJECT": 0}
    with tempfile.TemporaryDirectory() as tmp:
        stubs = {}
        for order in ("dec", "inc"):
            path = os.path.join(tmp, f"order_{order}.sail")
            with open(path, "w") as fh:
                fh.write(f"default Order {order}\n")
            stubs[order] = path

        # Which extras upstream can actually load, per vector order.
        libs = {}
        for order in ("dec", "inc"):
            drop = "vector_inc.sail" if order == "dec" else "vector_dec.sail"
            keep = [stubs[order], prelude]
            for extra in lib_all:
                if extra == prelude or os.path.basename(extra) == drop:
                    continue
                if run(args.sail, [*keep, extra], tmp):
                    keep.append(extra)
            # Both scopes are kept, and a file counts as `SCOPE` if *either*
            # accepts it. Adding library files is not monotone: they bring
            # their own definitions, and `test/ocaml/trycatch/tc.sail`
            # declares an exception type that `exception_basic.sail` already
            # declares — so the fuller scope rejects a file the prelude alone
            # accepts. Either way the rejection is about what the harness
            # supplies, not about the file.
            libs[order] = [[stubs[order], prelude], keep]
            print(
                f"  scope library ({order}): {len(keep) - 1} of {len(lib_all)} bundled files",
                file=sys.stderr,
            )

        for corpus in CORPORA:
            root = os.path.join(args.test_root, corpus)
            for dirpath, _, names in os.walk(root):
                for name in sorted(names):
                    if not name.endswith(".sail"):
                        continue
                    path = os.path.join(dirpath, name)
                    try:
                        text = open(path, encoding="utf-8", errors="replace").read()
                    except OSError:
                        continue
                    if run(args.sail, [name], dirpath):
                        verdict = "OK"
                    else:
                        order = declared_order(text)
                        verdict = (
                            "SCOPE"
                            if any(
                                run(args.sail, [*scope, name], dirpath)
                                for scope in libs[order]
                            )
                            else "REJECT"
                        )
                    counts[verdict] += 1
                    rows.append((verdict, path))
            print(f"  {corpus}: {len(rows)} files so far", file=sys.stderr)

    rows.sort(key=lambda r: r[1])
    version = subprocess.run([args.sail, "--version"], capture_output=True, text=True, check=True).stdout.strip()
    with open(args.out, "w") as fh:
        fh.write(f"# Upstream: {version}; SAIL_DIR={os.environ.get('SAIL_DIR', '(installed default)')}\n")
        fh.write(
            HEADER.format(
                corpora=",".join(CORPORA),
                ok=counts["OK"],
                scope=counts["SCOPE"],
                reject=counts["REJECT"],
            )
        )
        for verdict, path in rows:
            fh.write(f"{verdict:<7}{path}\n")

    print(
        f"{counts['OK']} OK, {counts['SCOPE']} SCOPE, {counts['REJECT']} REJECT"
        f" -> {args.out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
