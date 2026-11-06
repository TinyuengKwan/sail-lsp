#!/usr/bin/env python3
"""Create the same verified server payload consumed by bootstrap and VSIX.

Only build/publishing requires Python; installed servers and bootstraps do not.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
import re
from pathlib import Path
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGETS = json.loads((ROOT / "distribution/targets.json").read_text())


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checksum(path):
    Path(str(path) + ".sha256").write_text(f"{digest(path)}  {path.name}\n", encoding="utf-8")


def package(binary, destination, tag, allow_dirty=False):
    meta = json.loads(subprocess.check_output([str(binary.resolve()), "--version", "--json"], timeout=15))
    if meta["schemaVersion"] != 1 or meta["solver"] != "z3-vendored":
        raise ValueError("A distribution requires the versioned metadata interface and vendored Z3")
    if not allow_dirty and ("dirty" in meta["revision"] or "unknown" in meta["revision"]):
        raise ValueError("Release provenance must identify a clean source revision")
    if tag.removeprefix("v") != meta["version"]:
        raise ValueError(f"Tag {tag} does not match executable version {meta['version']}")
    target = next(t for t in TARGETS if t["target"] == meta["target"])
    if "windows" in meta["target"] and not meta.get("crtStatic", False):
        raise ValueError("Windows distributions must statically link the MSVC runtime (+crt-static)")
    requirements = target["requirements"].copy()
    if "linux" in meta["target"]:
        symbols = subprocess.check_output(["objdump", "-T", str(binary)], text=True)
        versions = re.findall(r"GLIBC_([0-9.]+)", symbols)
        measured = max(versions, key=lambda value:tuple(map(int,value.split('.')))) if versions else "0"
        if tuple(map(int,measured.split('.'))) > tuple(map(int,requirements["minimumGlibc"].split('.'))):
            if not allow_dirty:
                raise ValueError(f"Binary requires glibc {measured}, above the advertised {requirements['minimumGlibc']}; build on the release runner")
            requirements["minimumGlibc"] = measured
        requirements["observedGlibc"] = measured
    name = "sail-lsp.exe" if "windows" in meta["target"] else "sail-lsp"
    manifest = {**meta, "tag": tag, "binary": name, "binarySha256": digest(binary), "requirements": requirements}
    entries = [(name, binary.read_bytes(), 0o755)]
    entries.extend((p.name, p.read_bytes(), 0o755 if p.suffix == ".sh" else 0o644) for p in [ROOT / "LICENSE", ROOT / "install.sh", ROOT / "install.ps1"])
    for path in sorted((ROOT / "editors/zed").rglob("*")):
        relative = path.relative_to(ROOT)
        if path.is_file() and "target" not in relative.parts and ".git" not in relative.parts:
            entries.append((relative.as_posix(), path.read_bytes(), 0o644))
    entries.append(("manifest.json", (json.dumps(manifest, indent=2) + "\n").encode(), 0o644))
    destination.mkdir(parents=True, exist_ok=True)
    suffix = "zip" if "windows" in meta["target"] else "tar.gz"
    archive = destination / f"sail-lsp-{meta['target']}.{suffix}"
    if suffix == "zip":
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as out:
            for name, data, mode in entries:
                info = zipfile.ZipInfo(name, (2020, 1, 1, 0, 0, 0))
                info.external_attr = (0o100000 | mode) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                out.writestr(info, data)
    else:
        with archive.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w") as out:
            for name, data, mode in entries:
                info = tarfile.TarInfo(name)
                info.size, info.mode, info.mtime = len(data), mode, 0
                out.addfile(info, io.BytesIO(data))
    checksum(archive)
    descriptor = {**manifest, "archive": archive.name, "archiveSha256": digest(archive)}
    (destination / f"sail-lsp-{meta['target']}.json").write_text(json.dumps(descriptor, indent=2) + "\n")
    print(archive)
    return archive


def verify_and_extract(archive, destination):
    expected = Path(str(archive) + ".sha256").read_text().split()[0]
    if digest(archive) != expected:
        raise ValueError("Distribution checksum mismatch")
    destination.mkdir(parents=True, exist_ok=True)
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as src:
            for entry in src.infolist():
                if Path(entry.filename).is_absolute() or ".." in Path(entry.filename).parts or "\\" in entry.filename:
                    raise ValueError("Unsafe distribution entry")
            src.extractall(destination)
    else:
        with tarfile.open(archive) as src:
            src.extractall(destination, filter="data")
    manifest = json.loads((destination / "manifest.json").read_text())
    binary = destination / manifest["binary"]
    if digest(binary) != manifest["binarySha256"]:
        raise ValueError("Distribution binary checksum mismatch")
    binary.chmod(0o755)
    actual = json.loads(subprocess.check_output([str(binary.resolve()), "--version", "--json"], timeout=15))
    if any(actual[key] != manifest[key] for key in actual):
        raise ValueError("Distribution binary provenance mismatch")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    pack = sub.add_parser("pack")
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--output", type=Path, required=True)
    pack.add_argument("--tag", required=True)
    pack.add_argument("--allow-dirty", action="store_true", help="local validation only; never used in release workflows")
    extract = sub.add_parser("extract")
    extract.add_argument("archive", type=Path)
    extract.add_argument("destination", type=Path)
    sidecar = sub.add_parser("checksum")
    sidecar.add_argument("files", nargs="+", type=Path)
    plan = sub.add_parser("plan")
    plan.add_argument("--tag")
    args = parser.parse_args()
    if args.command == "pack":
        package(args.binary, args.output, args.tag, args.allow_dirty)
    elif args.command == "extract":
        verify_and_extract(args.archive, args.destination)
    elif args.command == "checksum":
        for file in args.files:
            checksum(file)
    else:
        version = tomllib.loads((ROOT / "crates/sail-lsp/Cargo.toml").read_text())["package"]["version"]
        editor_version = json.loads((ROOT / "editors/code/package.json").read_text())["version"]
        if version != editor_version or (args.tag and args.tag.removeprefix("v") != version):
            raise ValueError("Server, extension and release tag versions must agree")
        print(json.dumps({"include": TARGETS}, separators=(",", ":")))


if __name__ == "__main__":
    main()
