#!/usr/bin/env python3
"""Refresh the extended-corpus verdicts used by conformance_check.

Rows are OK (accepted standalone), SCOPE (accepted only with library scope), or
REJECT (rejected with either scope). Paths are relative to the supplied upstream
test directory. Verdicts use ordinary bitvector mode; scope-only inputs are not
scored as checker agreement. The compiler preflight must succeed first.

Usage: python3 scripts/regen_upstream_accepts.py /path/to/sail/test --sail sail
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
# Extended-corpus oracle for conformance_check.
# Paths are relative to the selected upstream test/ directory.
# Corpora: {corpora}.
# OK: accepts standalone. SCOPE: accepts with bundled-library scope only.
# REJECT: rejects with either scope. SCOPE is reported separately, not scored.
# {ok} OK, {scope} SCOPE, {reject} REJECT; ordinary bitvector mode.
# Regenerate with scripts/regen_upstream_accepts.py when compiler/library changes.
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
            "crates",
            "sail-lsp",
            "test_data",
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
                    rows.append((verdict, os.path.relpath(path, args.test_root).replace(os.sep, "/")))
            print(f"  {corpus}: {len(rows)} files so far", file=sys.stderr)

    rows.sort(key=lambda r: r[1])
    version = subprocess.run([args.sail, "--version"], capture_output=True, text=True, check=True).stdout.strip()
    with open(args.out, "w") as fh:
        fh.write(f"# Upstream: {version}\n")
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
