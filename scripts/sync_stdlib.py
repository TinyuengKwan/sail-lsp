#!/usr/bin/env python3
"""Synchronize the embedded Sail sources with a specific upstream checkout."""

import argparse
import pathlib
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sail", type=pathlib.Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    source = args.sail.resolve() / "lib"
    target = pathlib.Path(__file__).resolve().parents[1] / "crates/sail-lsp/data/sail-lib"
    if not (source / "prelude.sail").is_file():
        parser.error(f"not a Sail library: {source}")
    revision = subprocess.check_output(
        ["git", "-C", str(args.sail), "rev-parse", "HEAD"], text=True
    ).strip()
    expected = {p.relative_to(source): p.read_bytes() for p in source.rglob("*.sail")}
    existing = {p.relative_to(target): p for p in target.rglob("*.sail")}
    changed = [name for name, data in expected.items()
               if name not in existing or existing[name].read_bytes() != data]
    removed = existing.keys() - expected.keys()
    stamp = target.parent / "sail-lib-revision"
    stale_stamp = not stamp.exists() or stamp.read_text().strip() != revision
    if args.check:
        for name in sorted(changed):
            print(f"outdated or missing: {name}")
        for name in sorted(removed):
            print(f"removed upstream: {name}")
        if stale_stamp:
            print(f"revision must be {revision}")
        return int(bool(changed or removed or stale_stamp))
    for name in changed:
        dest = target / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(expected[name])
    for name in removed:
        existing[name].unlink()
    stamp.write_text(revision + "\n")
    print(f"Sail {revision}: {len(expected)} files, {len(changed)} updated, {len(removed)} removed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
