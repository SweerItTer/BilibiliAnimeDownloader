#!/usr/bin/env python3
"""Verify a GUI package against the shared update descriptor and its own manifest."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from verify_package import extract, package_root

TARGETS = {"x86_64-unknown-linux-gnu": "linux", "x86_64-pc-windows-msvc": "windows"}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_sums(root):
    sums = root / "SHA256SUMS"
    if not sums.is_file():
        raise SystemExit("package omits SHA256SUMS")
    seen = set()
    for line in sums.read_text(encoding="ascii").splitlines():
        match = line.split("  ", 1)
        if len(match) != 2:
            raise SystemExit("invalid internal SHA256SUMS entry")
        digest, relative = match
        if relative in seen:
            raise SystemExit("duplicate internal SHA256SUMS entry")
        seen.add(relative)
        path = root / relative
        if not path.is_file() or sha256(path) != digest:
            raise SystemExit(f"internal checksum mismatch: {relative}")
    for path in root.rglob("*"):
        if path.is_file() and path.name != "SHA256SUMS":
            if path.relative_to(root).as_posix() not in seen:
                raise SystemExit(f"file missing from internal SHA256SUMS: {path.relative_to(root)}")


def version_of(root, kind):
    executable = root / ("gui.exe" if kind == "windows" else "bin/gui")
    if not executable.is_file():
        raise SystemExit("package omits the GUI executable")
    environment = os.environ.copy()
    environment.pop("LD_LIBRARY_PATH", None)
    output = subprocess.run(
        [str(executable), "--version"],
        check=True,
        capture_output=True,
        text=True,
        env=environment,
    ).stdout.strip()
    prefix = "Bili "
    if not output.startswith(prefix):
        raise SystemExit(f"unexpected version output: {output!r}")
    return output[len(prefix):]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("descriptor", type=Path)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    kind = TARGETS[args.target]
    descriptor = json.loads(args.descriptor.read_text(encoding="utf-8"))
    archive = descriptor.get("archive", {})
    if descriptor.get("schema_version") != 1:
        raise SystemExit("descriptor schema_version must be 1")
    if descriptor.get("product") != "gui" or descriptor.get("target") != args.target:
        raise SystemExit("descriptor product/target does not match the GUI package")
    if archive.get("name") != args.package.name:
        raise SystemExit("descriptor archive name does not match the package")
    if archive.get("format") != ("zip" if kind == "windows" else "tar.gz"):
        raise SystemExit("descriptor archive format does not match the package")
    if archive.get("size") != args.package.stat().st_size:
        raise SystemExit("descriptor archive size does not match the package")
    if archive.get("sha256") != sha256(args.package):
        raise SystemExit("descriptor archive hash does not match the package")

    with tempfile.TemporaryDirectory(prefix="bili-gui-verify-") as temporary:
        try:
            extract(args.package, Path(temporary))
            root = package_root(Path(temporary))
        except ValueError as error:
            raise SystemExit(str(error)) from error
        verify_sums(root)
        build = json.loads((root / "BUILD_INFO.json").read_text(encoding="utf-8"))
        if build.get("product") != "gui" or build.get("target") != args.target:
            raise SystemExit("BUILD_INFO does not describe this GUI target")
        if build.get("app_version") != descriptor.get("version"):
            raise SystemExit("BUILD_INFO version differs from the descriptor")
        reported = version_of(root, kind)
        if reported != descriptor.get("version"):
            raise SystemExit("packaged executable reports a different version")

    report = {
        "status": "passed",
        "product": "gui",
        "target": args.target,
        "version": descriptor.get("version"),
        "package_sha256": archive.get("sha256"),
    }
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
