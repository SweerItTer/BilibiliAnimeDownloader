#!/usr/bin/env python3
"""Collect complete, source-provenance dependency license texts for a release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import urllib.request
import subprocess
import sys
import tempfile

import tarfile
import tomllib
ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PREFIX = "registry+https://github.com/rust-lang/crates.io-index"
LICENSE_IDS = frozenset({
    "0BSD", "Apache-2.0", "Artistic-2.0", "BSD-2-Clause", "BSD-3-Clause",
    "BSL-1.0", "CDLA-Permissive-2.0", "ISC", "MIT", "MPL-2.0", "OpenSSL",
    "Unicode-3.0", "Unicode-DFS-2016", "Unlicense", "WTFPL", "Zlib",
})
LICENSE_FILES = {"license", "copying", "notice", "unlicense", "copyright"}
TARGET_RE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.-]*$")




def parse_expression(expression):
    """Validate SPDX and legacy Cargo slash-separated OR expressions."""
    normalized = expression.strip()
    if "/" in normalized and not re.search(r"[()\s]|\b(?:AND|OR|WITH)\b", normalized):
        alternatives = normalized.split("/")
        if all(item in LICENSE_IDS for item in alternatives):
            normalized = " OR ".join(alternatives)
    tokens = re.findall(r"\(|\)|\bAND\b|\bOR\b|\bWITH\b|[^\s()]+", normalized)
    if "".join(tokens).replace("AND", "").replace("OR", "").replace("WITH", "") == "":
        raise ValueError("empty license expression")
    pos = 0

    def primary():
        nonlocal pos
        if pos >= len(tokens):
            raise ValueError("incomplete license expression")
        if tokens[pos] == "(":
            pos += 1
            either()
            if pos >= len(tokens) or tokens[pos] != ")":
                raise ValueError("unclosed parenthesis in license expression")
            pos += 1
            return
        identifier = tokens[pos]
        if identifier not in LICENSE_IDS:
            raise ValueError(f"unsupported or unknown SPDX license identifier: {identifier}")
        pos += 1
        if pos < len(tokens) and tokens[pos] == "WITH":
            pos += 1
            if pos >= len(tokens) or not re.fullmatch(r"[A-Za-z0-9.-]+", tokens[pos]):
                raise ValueError("invalid SPDX exception identifier")
            # No exception catalogue is bundled: unknown exception semantics fail closed.
            if tokens[pos] not in {"LLVM-exception", "Autoconf-exception-3.0", "Classpath-exception-2.0", "GCC-exception-3.1"}:
                raise ValueError(f"unsupported SPDX exception identifier: {tokens[pos]}")
            pos += 1

    def conjunction():
        nonlocal pos
        primary()
        while pos < len(tokens) and tokens[pos] == "AND":
            pos += 1
            primary()

    def either():
        nonlocal pos
        conjunction()
        while pos < len(tokens) and tokens[pos] == "OR":
            pos += 1
            conjunction()

    either()
    if pos != len(tokens):
        raise ValueError(f"invalid SPDX license expression near {tokens[pos]!r}")


def selected_tree(metadata, package_name, target):
    text = subprocess.check_output([
        "cargo", "tree", "--locked", "--package", package_name, "--target", target,
        "--edges", "normal", "--prefix", "none", "--format", "{p}",
    ], cwd=ROOT, text=True, encoding="utf-8")
    selected = set()
    for line in text.splitlines():
        match = re.match(r"^([^\s]+) v([^\s]+)", line.strip())
        if not match:
            raise ValueError(f"unrecognized cargo tree package row: {line!r}")
        selected.add((match.group(1), match.group(2)))
    packages = metadata["packages"]
    counts = {}
    for package in packages:
        key = (package["name"], package["version"])
        counts[key] = counts.get(key, 0) + 1
    ambiguous = sorted(key for key in selected if counts.get(key, 0) > 1)
    if ambiguous:
        raise ValueError(f"cargo tree output ambiguously matches package identities: {ambiguous}")
    return selected


def lock_checksums():
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    return {
        (package["name"], package["version"], package.get("source")): package.get("checksum")
        for package in lock["package"] if package.get("source", "").startswith(REGISTRY_PREFIX)
    }


def registry_archive(registry, name, version, expected):
    archive = registry.parent.parent / "cache" / registry.name / f"{name}-{version}.crate"
    if not archive.is_file():
        raise ValueError(f"Cargo registry archive is missing for {name} {version}")
    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    if digest.hexdigest() != expected:
        raise ValueError(f"Cargo registry archive checksum mismatch for {name} {version}")
    return archive


def verify_archive_license_texts(archive, crate_root, name, version, texts):
    prefix = f"{name}-{version}/"
    verified = {}
    with tarfile.open(archive, "r:gz") as tar:
        for path in texts:
            relative = path.relative_to(crate_root).as_posix()
            member_name = prefix + relative
            try:
                member = tar.getmember(member_name)
            except KeyError as error:
                raise ValueError(f"license text is absent from checksum-verified crate archive: {relative}") from error
            if not member.isfile():
                raise ValueError(f"license source is not a regular archive file: {relative}")
            source = tar.extractfile(member)
            if source is None:
                raise ValueError(f"cannot read license text from crate archive: {relative}")
            data = source.read()
            if data != path.read_bytes():
                raise ValueError(f"unpacked license text differs from checksum-verified crate archive: {relative}")
            verified[path] = data
    return verified


def normal_closure(metadata, package_name, selected):
    matches = [package for package in metadata["packages"] if package["name"] == package_name]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one package named {package_name}, found {len(matches)}")
    root_id = matches[0]["id"]
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    if root_id not in nodes:
        raise ValueError(f"package {package_name} is absent from Cargo's resolved graph")
    reached, pending = set(), [root_id]
    while pending:
        package_id = pending.pop()
        if package_id in reached:
            continue
        reached.add(package_id)
        node = nodes.get(package_id)
        if node is None:
            raise ValueError(f"Cargo graph references missing package node {package_id}")
        for dep in node["deps"]:
            if any(kind.get("kind") is None for kind in dep.get("dep_kinds", [])):
                pending.append(dep["pkg"])
    by_id = {package["id"]: package for package in metadata["packages"]}
    workspace_members = set(metadata["workspace_members"])
    return [by_id[item] for item in sorted(reached)
            if (by_id[item]["name"], by_id[item]["version"]) in selected and
            (by_id[item].get("source") or item in workspace_members)]


def registry_root(manifest_path):
    manifest = Path(manifest_path).resolve(strict=True)
    parts = manifest.parts
    try:
        marker = parts.index("registry")
        if parts[marker + 1] != "src":
            raise ValueError
        root = Path(*parts[:marker + 3])
    except (ValueError, IndexError):
        raise ValueError(f"dependency source is outside Cargo registry source roots: {manifest}")
    if manifest.parent != root / (manifest.parent.name):
        raise ValueError(f"invalid registry crate root: {manifest.parent}")
    return root, manifest.parent


def source_files(crate_root, license_file):
    files = []
    if license_file:
        candidate = (crate_root / license_file).resolve(strict=True)
        if not candidate.is_file() or not candidate.is_relative_to(crate_root):
            raise ValueError(f"Cargo license_file escapes crate root or is not a file: {license_file}")
        files.append(candidate)
    for candidate in crate_root.iterdir():
        if not candidate.is_file():
            continue
        stem = candidate.name.casefold().split(".", 1)[0]
        if stem in LICENSE_FILES or any(stem.startswith(name + "-") or stem.startswith(name + "_") for name in LICENSE_FILES):
            resolved = candidate.resolve(strict=True)
            if not resolved.is_relative_to(crate_root):
                raise ValueError(f"license text symlink escapes crate root: {candidate.name}")
            files.append(resolved)
    files = sorted(set(files), key=lambda path: (path.name.casefold(), path.name))
    if len({path.name.casefold() for path in files}) != len(files):
        raise ValueError(f"case-insensitive license filename collision in {crate_root.name}")
    return files


def copy_text(source, dest, output_root, provenance, data=None):
    data = source.read_bytes() if data is None else data
    try:
        data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError(f"license text is not UTF-8 ({provenance}): {source.name}") from error
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)
    return {"path": dest.relative_to(output_root).as_posix(), "sha256": hashlib.sha256(data).hexdigest(), "provenance": provenance}


def pinned_declared_license(package, version, declaration, output_root):
    lock = json.loads((ROOT / "scripts/dependency-license-lock.json").read_text(encoding="utf-8"))
    matches = [source for source in lock.get("sources", [])
               if source.get("package") == package and source.get("version") == version]
    if len(matches) != 1 or matches[0].get("spdx") != declaration:
        raise ValueError(f"no exact pinned full-text source for {package} {version} ({declaration})")
    source = matches[0]
    if (not source.get("url", "").startswith("https://") or
            not re.fullmatch(r"[0-9a-f]{64}", source.get("sha256", "")) or
            Path(source.get("filename", "")).name != source.get("filename")):
        raise ValueError(f"invalid pinned license source for {package} {version}")
    cache = ROOT / "target/license-cache"
    cached = cache / f"{package}-{version}-{source['filename']}"
    if cached.exists():
        data = cached.read_bytes()
    else:
        request = urllib.request.Request(source["url"], headers={"User-Agent": "BilibiliAnimeDownloader-license-collector/1"})
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != source["sha256"]:
            raise ValueError(f"downloaded license source checksum mismatch for {package} {version}")
        cache.mkdir(parents=True, exist_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=f".{cached.name}.", dir=cache)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(data)
            try:
                os.link(temporary, cached)
            except FileExistsError:
                data = cached.read_bytes()
        finally:
            Path(temporary).unlink(missing_ok=True)
    if hashlib.sha256(data).hexdigest() != source["sha256"]:
        raise ValueError(f"cached license source checksum mismatch for {package} {version}")
    destination = output_root / "licenses" / "crates" / f"{package}-{version}" / source["filename"]
    text_record = copy_text(Path(source["filename"]), destination, output_root,
                            f"pinned canonical SPDX license text: {source['url']}", data)
    return text_record, {
        "url": source["url"], "sha256": source["sha256"], "provenance": source["provenance"],
    }


def runtime_root(target):
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot", "--target", target], text=True).strip()).resolve(strict=True)
    candidates = [sysroot / "share/doc/rust/licenses", sysroot / "share/doc/rust/html/licenses"]
    for candidate in candidates:
        if candidate.is_dir():
            return sysroot, candidate
    raise ValueError(f"Rust sysroot contains no installed license directory: {sysroot}")


def collect(args):

    if not TARGET_RE.fullmatch(args.target):
        raise ValueError(f"invalid Rust target triple: {args.target}")
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--filter-platform", args.target, "--format-version", "1"
    ], cwd=ROOT, text=True, encoding="utf-8"))
    selected = selected_tree(metadata, args.package, args.target)
    packages = normal_closure(metadata, args.package, selected)
    checksums = lock_checksums()
    workspace_members = set(metadata["workspace_members"])
    records = []
    output_root = Path(args.stage)
    for package in packages:
        source = package.get("source")
        if package["id"] in workspace_members:
            project_license = ROOT / "LICENSE"
            if not project_license.is_file():
                raise ValueError("repository LICENSE is missing for first-party workspace components")
            prefix = f"licenses/project/{package['name']}-{package['version']}"
            records.append({
                "package_id": package["id"], "name": package["name"], "version": package["version"],
                "source": None, "checksum": None, "spdx": None,
                "license_basis": "repository LICENSE applies to this first-party workspace component; no SPDX identifier is asserted",
                "texts": [copy_text(project_license, output_root / prefix / "LICENSE", output_root,
                                    "repository LICENSE, first-party workspace component")],
            })
            continue
        if not source or not source.startswith(REGISTRY_PREFIX):
            raise ValueError(f"unsupported dependency source for {package['name']} {package['version']}: {source}")
        registry, crate_root = registry_root(package["manifest_path"])
        if not crate_root.is_relative_to(registry):
            raise ValueError(f"registry crate escaped registry source root: {crate_root}")
        checksum = checksums.get((package["name"], package["version"], source))
        if not checksum:
            raise ValueError(f"Cargo.lock lacks registry checksum for {package['name']} {package['version']}")
        archive = registry_archive(registry, package["name"], package["version"], checksum)
        declaration = package.get("license")
        if not declaration:
            raise ValueError(f"missing license declaration for {package['name']} {package['version']}")
        try:
            parse_expression(declaration)
        except ValueError as error:
            raise ValueError(
                f"unsupported license declaration for {package['name']} {package['version']} "
                f"({declaration!r}): {error}"
            ) from error
        texts = source_files(crate_root, package.get("license_file"))
        if not texts:
            text_record, license_source = pinned_declared_license(
                package["name"], package["version"], declaration, output_root)
            records.append({
                "package_id": package["id"], "name": package["name"], "version": package["version"],
                "source": source, "checksum": checksum, "spdx": declaration,
                "license_sources": [license_source], "texts": [text_record],
            })
            continue
        verified_texts = verify_archive_license_texts(archive, crate_root, package["name"], package["version"], texts)
        prefix = f"licenses/crates/{package['name']}-{package['version']}"
        records.append({
            "package_id": package["id"], "name": package["name"], "version": package["version"],
            "source": source, "checksum": checksum, "spdx": declaration,
            "texts": [copy_text(path, output_root / prefix / path.name, output_root,
                                f"Cargo registry archive {path.relative_to(crate_root).as_posix()}",
                                verified_texts[path]) for path in texts],
        })
    if args.rust_runtime:
        sysroot, license_dir = runtime_root(args.target)
        doc_root = sysroot / "share/doc/rust"
        runtime_sources = []
        if doc_root.is_dir():
            for path in source_files(doc_root, None):
                runtime_sources.append((path, Path("root") / path.name))
        runtime_sources.extend((path, Path("licenses") / path.relative_to(license_dir))
                               for path in license_dir.rglob("*") if path.is_file())
        if not runtime_sources:
            raise ValueError(f"Rust sysroot contains no installed license texts: {sysroot}")
        license_root = license_dir.resolve(strict=True)
        if any(not path.resolve(strict=True).is_relative_to(license_root) for path, _ in runtime_sources
               if path.is_relative_to(license_dir)):
            raise ValueError("Rust sysroot license asset symlink escapes the installed license directory")
        records.append({
            "name": "rust-std", "version": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "checksum": None, "spdx": None,
            "texts": [copy_text(path, output_root / "licenses/rust-runtime" / relative, output_root,
                                f"rustc sysroot {path.relative_to(sysroot).as_posix()}") for path, relative in runtime_sources],
        })
    for index, source in enumerate(args.runtime_license):
        path = source.resolve(strict=True)
        if not path.is_file():
            raise ValueError(f"runtime license input is not a file: {path.name}")
        records.append({
            "name": f"runtime-license-{index + 1}", "version": None, "checksum": None, "spdx": None,
            "texts": [copy_text(path, output_root / "licenses/runtime-inputs" / f"{index + 1:03d}-{path.name}",
                                output_root, f"caller-supplied runtime license file {path.name}")],
        })
    index_path = output_root / "index.json"
    index_path.write_text(json.dumps({"format": 1, "package": args.package, "target": args.target,
                                      "dependencies": records}, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", choices=("core-cli", "bili_core"), required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rust-runtime", action="store_true")
    parser.add_argument("--runtime-license", type=Path, action="append", default=[])
    args = parser.parse_args()
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        parser.error(f"output must not already exist: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    lock_path = output.with_name(output.name + ".lock")
    try:
        lock_fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        parser.error(f"license output is locked by another collector: {lock_path}")
    os.close(lock_fd)
    try:
        if output.exists() or output.is_symlink():
            parser.error(f"output must not already exist: {output}")
        stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
        args.stage = stage
        try:
            collect(args)
            if output.exists() or output.is_symlink():
                raise FileExistsError(f"output appeared during collection: {output}")
            os.rename(stage, output)
        except Exception as error:
            shutil.rmtree(stage, ignore_errors=True)
            print(f"license collection failed: {error}", file=sys.stderr)
            return 1
    finally:
        lock_path.unlink(missing_ok=True)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
