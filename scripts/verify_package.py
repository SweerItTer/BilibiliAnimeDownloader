#!/usr/bin/env python3
"""Safely extract and inspect a Bilibili package; optionally run opt-in public-media recovery smoke."""
import atexit
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import queue
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import uuid
import zipfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SYSTEM_LINUX = re.compile(r"^(?:libc\.so\.6|libm\.so\.6|libgcc_s\.so\.1|libpthread\.so\.0|libdl\.so\.2|librt\.so\.1|libutil\.so\.1|libresolv\.so\.2|ld-linux-x86-64\.so\.2)$")
FORBIDDEN_NAMES = {"ffmpeg", "ffmpeg.exe", "ffprobe", "ffprobe.exe", "aria2c", "mpv", "vlc", "mplayer"}
WINDOWS_SYSTEM = {
    "kernel32.dll", "kernelbase.dll", "user32.dll", "advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll",
    "crypt32.dll", "ntdll.dll", "ws2_32.dll", "shell32.dll", "ole32.dll", "oleaut32.dll", "secur32.dll",
    "iphlpapi.dll", "version.dll", "winmm.dll", "gdi32.dll", "comdlg32.dll", "normaliz.dll", "msvcrt.dll",
    "ucrtbase.dll", "shlwapi.dll", "combase.dll", "imm32.dll", "wintrust.dll", "psapi.dll", "userenv.dll",
    "profapi.dll", "cfgmgr32.dll", "rpcrt4.dll", "winhttp.dll", "dnsapi.dll",
}


def has_forbidden_native_options(text):
    option = r"CONFIG_(?:GPL|NONFREE|VERSION3|FFMPEG|FFPROBE)"
    return re.search(
        rf"^[ \t]*(?:{option}[ \t]*=[ \t]*yes[ \t]*(?:#.*)?$|"
        rf"#[ \t]*define[ \t]+{option}[ \t]+1(?:[ \t]|$))",
        text,
        re.I | re.M,
    ) is not None


def sha256(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def safe_member(name):
    if "\\" in name or ":" in name:
        raise ValueError(f"unsafe archive path: {name}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"unsafe archive path: {name}")
    return path


def safe_target(root, rel):
    root = root.resolve()
    target = root.joinpath(*rel.parts)
    resolved = target.resolve(strict=False)
    if not resolved.is_relative_to(root):
        raise ValueError(f"archive member escapes extraction root: {rel}")
    return target


def extract(archive, destination):
    destination = destination.resolve()
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as bundle:
            for member in bundle.infolist():
                name = member.filename.rstrip("/")
                if not name:
                    continue
                target = safe_target(destination, safe_member(name))
                mode = member.external_attr >> 16
                kind = mode & 0o170000
                if kind == 0o120000 or kind not in (0, 0o100000, 0o040000):
                    raise ValueError(f"unsupported ZIP entry: {member.filename}")
                if member.is_dir():
                    if target.exists() and not target.is_dir():
                        raise ValueError(f"duplicate archive path: {member.filename}")
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    if target.exists() or target.is_symlink():
                        raise ValueError(f"duplicate archive path: {member.filename}")
                    with bundle.open(member) as src, target.open("xb") as dst:
                        shutil.copyfileobj(src, dst)
                    if mode:
                        target.chmod(mode & 0o777)
        return
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle.getmembers():
            name = member.name.rstrip("/")
            if not name:
                continue
            rel = safe_member(name)
            target = safe_target(destination, rel)
            if member.isdir():
                if target.exists() and not target.is_dir():
                    raise ValueError(f"duplicate archive path: {member.name}")
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.exists() or target.is_symlink():
                    raise ValueError(f"duplicate archive path: {member.name}")
                stream = bundle.extractfile(member)
                if stream is None:
                    raise ValueError(f"cannot read archive member {member.name}")
                with stream, target.open("xb") as dst:
                    shutil.copyfileobj(stream, dst)
                target.chmod(member.mode & 0o777)
            elif member.issym():
                if ":" in member.linkname or "\\" in member.linkname:
                    raise ValueError(f"unsafe symbolic link: {member.name}")
                link = PurePosixPath(member.linkname)
                if link.is_absolute():
                    raise ValueError(f"unsafe symbolic link: {member.name}")
                if not (target.parent / Path(*link.parts)).resolve(strict=False).is_relative_to(destination):
                    raise ValueError(f"symbolic link escapes package: {member.name}")
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.exists() or target.is_symlink():
                    raise ValueError(f"duplicate archive path: {member.name}")
                target.symlink_to(member.linkname)
            else:
                raise ValueError(f"unsupported archive entry: {member.name}")


def package_root(extracted):
    roots = [path for path in extracted.iterdir() if path.is_dir()]
    if len(roots) != 1:
        raise ValueError("archive must contain exactly one package root")
    return roots[0]


def check_checksums(root):
    sums = root / "SHA256SUMS"
    if not sums.is_file():
        raise ValueError(f"missing SHA256SUMS in {root}")
    expected = {}
    for line in sums.read_text(encoding="ascii").splitlines():
        digest, sep, rel = line.partition("  ")
        if not sep or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError("malformed SHA256SUMS entry")
        member = safe_member(rel)
        if rel in expected:
            raise ValueError(f"duplicate checksum path: {rel}")
        expected[rel] = (member, digest)
    actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file() and p.name != "SHA256SUMS"}
    if actual != set(expected):
        raise ValueError(f"checksum file set mismatch; unlisted={sorted(actual - set(expected))}, missing={sorted(set(expected) - actual)}")
    for rel, (member, digest) in expected.items():
        if sha256(root.joinpath(*member.parts)) != digest:
            raise ValueError(f"checksum mismatch: {rel}")


def validate_dependency_licenses(root, target):
    index_path = root / "DEPENDENCY_LICENSES.json"
    if not index_path.is_file():
        raise ValueError("package lacks its complete dependency-license index")
    index = json.loads(index_path.read_text(encoding="utf-8"))
    records = index.get("dependencies")
    if index.get("format") != 1 or index.get("package") != "core-cli" or index.get("target") != target or not isinstance(records, list) or not records:
        raise ValueError("dependency-license index has the wrong package/target or no dependency records")
    text_count = 0
    for record in records:
        texts = record.get("texts") if isinstance(record, dict) else None
        if not isinstance(texts, list) or not texts:
            raise ValueError("dependency-license record has no source license texts")
        for text in texts:
            if not isinstance(text, dict) or not isinstance(text.get("path"), str):
                raise ValueError("dependency-license text record is malformed")
            member = safe_member(text["path"])
            path = safe_target(root, member)
            if not member.parts or member.parts[0] != "licenses" or not path.is_file() or path.is_symlink():
                raise ValueError("dependency license text is missing or outside licenses/")
            if sha256(path) != text.get("sha256"):
                raise ValueError(f"dependency license text checksum mismatch: {text['path']}")
            text_count += 1
    if not text_count:
        raise ValueError("package contains no full dependency-license texts")


def validate_windows_runtime(root, build_info, target):
    if target != "x86_64-pc-windows-msvc":
        return
    for key in ("windows_msvc_toolset_version", "windows_sdk_version", "windows_vc_redist_directory"):
        if not isinstance(build_info.get(key), str) or not build_info[key].strip():
            raise ValueError(f"Windows build tuple lacks {key}")
    entries = build_info.get("windows_vc_runtime_dlls")
    if not isinstance(entries, list):
        raise ValueError("Windows build tuple lacks the copied VC runtime DLL inventory")
    actual = {
        path.name.lower(): path for path in root.glob("*.dll")
        if path.name.lower().startswith(("vcruntime", "msvcp", "concrt"))
    }
    declared = {}
    for entry in entries:
        name = entry.get("name") if isinstance(entry, dict) else None
        digest = entry.get("sha256") if isinstance(entry, dict) else None
        if (not isinstance(name, str) or Path(name).name != name or
                not name.lower().startswith(("vcruntime", "msvcp", "concrt")) or
                name.lower() in declared or not isinstance(digest, str) or
                not re.fullmatch(r"[0-9a-f]{64}", digest)):
            raise ValueError("Windows VC runtime DLL inventory contains a malformed entry")
        declared[name.lower()] = digest
    if set(actual) != set(declared) or any(sha256(actual[name]) != digest for name, digest in declared.items()):
        raise ValueError("Windows build tuple does not match the actual packaged VC runtime DLLs")
    notices = root / "licenses/windows-redist"
    if not actual:
        if notices.exists():
            raise ValueError("Windows terms were included without a packaged VC runtime closure")
        return
    provenance_path = notices / "provenance.json"
    if not provenance_path.is_file():
        raise ValueError("packaged VC runtime is missing its pinned Microsoft terms provenance")
    lock = json.loads((ROOT / "scripts/windows-redist-lock.json").read_text(encoding="utf-8"))
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    sources = provenance.get("sources")
    if provenance.get("format") != 1 or not isinstance(sources, list):
        raise ValueError("Microsoft terms provenance has an unsupported schema")
    source_records = {item.get("id"): item for item in sources if isinstance(item, dict)}
    if set(source_records) != {item["id"] for item in lock.get("sources", [])}:
        raise ValueError("Microsoft terms provenance does not cover the pinned official documents")
    for pin in lock["sources"]:
        record = source_records[pin["id"]]
        if record.get("source_sha256") != pin["sha256"]:
            raise ValueError(f"Microsoft source digest differs from the pin: {pin['id']}")
        source_member = safe_member(record.get("source_path", ""))
        source_file = safe_target(notices, source_member)
        if not source_file.is_file() or sha256(source_file) != pin["sha256"]:
            raise ValueError(f"pinned Microsoft source document is missing or changed: {pin['id']}")
        text_path = record.get("text_path")
        if text_path is not None:
            text_member = safe_member(text_path)
            text_file = safe_target(notices, text_member)
            if not text_file.is_file() or sha256(text_file) != record.get("text_sha256"):
                raise ValueError(f"extracted Microsoft terms text is missing or changed: {pin['id']}")

def validate_config(native):
    lock = json.loads((ROOT / "scripts/native-lock.json").read_text(encoding="utf-8"))
    info = json.loads((native / "build-info.json").read_text(encoding="utf-8"))
    bundled_lock = json.loads((native / "source/native-lock.json").read_text(encoding="utf-8"))
    if bundled_lock != lock or info.get("source") != lock:
        raise ValueError("bundled native pin differs from the checked source pin")
    if info.get("target") not in ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"):
        raise ValueError("unsupported native build target")
    if info.get("source", {}).get("version") != "8.0.3" or info.get("license") != "LGPL-2.1-or-later":
        raise ValueError("unacceptable FFmpeg source version or license")
    if info.get("patches") != []:
        raise ValueError("native build records patches absent from the corresponding-source recipe")
    effective = info.get("configure", [])
    if not all(flag in effective for flag in lock["configure"]):
        raise ValueError("effective FFmpeg configure flags differ from pinned configuration")
    forbidden = ("--enable-gpl", "--enable-nonfree", "--enable-version3", "--enable-static", "--enable-programs")
    if any(flag == bad or flag.startswith(bad + "=") for flag in effective for bad in forbidden):
        raise ValueError("forbidden GPL/nonfree/version-3/static/program configuration")
    for name in ("config.h", "config_components.h", "config.mak"):
        text = (native / name).read_text(encoding="utf-8", errors="replace")
        if has_forbidden_native_options(text):
            raise ValueError(f"forbidden FFmpeg option enabled in {name}")
    if not (native / "COPYING.LGPLv2.1").is_file() or not (native / "LICENSE.md").is_file():
        raise ValueError("native LGPL license notices missing")
    return info, lock


def inspect_native(root, target, source_asset, source_extract):
    native = root / "share/bili/native"
    info, lock = validate_config(native)
    if info.get("target") != target:
        raise ValueError("native package target mismatch")
    app_license = (root / "LICENSE").read_text(encoding="utf-8")
    if not app_license.startswith("MIT License with Commons Clause") or "Commons Clause Restriction" not in app_license:
        raise ValueError("application Commons Clause license is missing or misrepresented")
    reference = {}
    for line in (native / "source/FFMPEG_SOURCE.txt").read_text(encoding="utf-8").splitlines():
        key, sep, value = line.partition("=")
        if not sep or key in reference:
            raise ValueError("malformed native source reference")
        reference[key] = value
    if reference.get("source_asset") != source_asset.name or source_asset.name != f"bili-native-source-{target}.tar.gz":
        raise ValueError("the corresponding native source archive is missing or mismatched")
    if sha256(source_asset) != reference.get("asset_sha256"):
        raise ValueError("native source archive asset checksum mismatch")
    if reference.get("ffmpeg_archive") != lock["archive"] or reference.get("ffmpeg_sha256") != lock["sha256"]:
        raise ValueError("native source reference does not match the FFmpeg pin")
    source_extract.mkdir(mode=0o700)
    extract(source_asset, source_extract)
    source_root = package_root(source_extract)
    check_checksums(source_root)
    source_native = source_root / "share/bili/native"
    source_info = json.loads((source_native / "build-info.json").read_text(encoding="utf-8"))
    if source_info != info:
        raise ValueError("native source bundle build tuple does not match runtime package")
    source_lock = json.loads((source_native / "source/scripts/native-lock.json").read_text(encoding="utf-8"))
    if source_lock != lock:
        raise ValueError("native source bundle pin mismatch")
    upstream = source_native / "source" / lock["archive"]
    if sha256(upstream) != lock["sha256"]:
        raise ValueError("bundled upstream FFmpeg archive checksum mismatch")
    recipe = source_native / "source/scripts/build_ffmpeg.py"
    if sha256(recipe) != info.get("recipe_sha256"):
        raise ValueError("bundled native build recipe differs from build tuple")
    for name in ("config.h", "config_components.h", "config.mak", "COPYING.LGPLv2.1", "LICENSE.md"):
        if sha256(source_native / name) != sha256(native / name):
            raise ValueError(f"native source bundle does not match runtime {name}")
    if not (source_root / "THIRD_PARTY_NOTICES.txt").is_file() or not (source_native / "source/BUILDING.txt").is_file():
        raise ValueError("native source bundle lacks notices or build instructions")
    return info


def reject_dynamic_rust_core(dependencies):
    bad = [name for name in dependencies if name.lower().startswith(("bili_core", "libbili_core", "rust_std", "libstd-"))]
    if bad:
        raise ValueError(f"ordinary CLI unexpectedly depends on dynamically linked Rust core/runtime: {sorted(bad)}")


def run(command, env, cwd, *, encoding=None, deadline=None):
    timeout = None if deadline is None else max(0.001, deadline - time.monotonic())
    result = subprocess.run(command, cwd=cwd, env=env, text=True, encoding=encoding,
                            capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {command[0]} {command[1:]}; "
            f"stdout: {result.stdout.strip()}\nstderr: {result.stderr.strip()}"
        )
    return result.stdout + result.stderr


def inspect_linux(root, env):
    app = root / "bin/bili"
    dynamic = run(["readelf", "-d", str(app)], env, root)
    if "$ORIGIN/../lib" not in dynamic:
        raise ValueError("Linux app lacks required app-relative RUNPATH")
    needed = set(re.findall(r"Shared library: \[(.*?)\]", dynamic))
    reject_dynamic_rust_core(needed)
    files = [app, *sorted((root / "lib").glob("*.so*"))]
    package_libs = {p.name for p in (root / "lib").glob("*.so*")}
    for base in ("libavformat.so.62", "libavcodec.so.62", "libavutil.so.60"):
        if not any((root / "lib").glob(base + "*")):
            raise ValueError(f"missing required package library {base}")
    for file in files:
        if file.is_symlink():
            continue
        ldd = run(["ldd", str(file)], env, root)
        if "not found" in ldd:
            raise ValueError(f"unresolved runtime dependency in {file.name}: {ldd}")
        for line in ldd.splitlines():
            match = re.match(r"\s*(\S+)\s+=>\s+(\S+)", line)
            if not match:
                continue
            name, resolved = match.groups()
            if name in package_libs:
                if not Path(resolved).resolve().is_relative_to((root / "lib").resolve()):
                    raise ValueError(f"{name} resolved outside package: {resolved}")
            elif not SYSTEM_LINUX.fullmatch(name):
                raise ValueError(f"unapproved non-package Linux dependency: {name} -> {resolved}")
        if file != app:
            rpath = run(["readelf", "-d", str(file)], env, root)
            if "$ORIGIN" not in rpath:
                raise ValueError(f"library lacks own-directory RUNPATH: {file.name}")


def parse_windows_dependencies(output):
    return re.findall(r"^\s*([A-Za-z0-9_.-]+\.dll)\s*$", output, re.M | re.I)


def inspect_windows(root, env, executable="bili.exe"):
    dumpbin = shutil.which("dumpbin")
    if not dumpbin:
        raise ValueError("dumpbin is required for Windows import inspection")
    files = [root / executable, *sorted(root.glob("*.dll"))]
    bundled = {p.name.lower() for p in root.glob("*.dll")}
    names = bundled
    required = ({"avformat-62.dll", "libavformat-62.dll"}, {"avcodec-62.dll", "libavcodec-62.dll"}, {"avutil-60.dll", "libavutil-60.dll"})
    if not all(names & variants for variants in required):
        raise ValueError(f"Windows package lacks required FFmpeg DLLs beside {executable}")
    for file in files:
        output = run([dumpbin, "/nologo", "/dependents", str(file)], env, root)
        deps = {name.lower() for name in parse_windows_dependencies(output)}
        reject_dynamic_rust_core(deps)
        unresolved = {name for name in deps - bundled - WINDOWS_SYSTEM if not name.startswith(("api-ms-win-", "ext-ms-win-"))}
        if unresolved:
            raise ValueError(f"unbundled Windows dependency in {file.name}: {sorted(unresolved)}")


def validate_replacement(prefix, root, target, env):
    info_path = prefix / "build-info.json"
    info = json.loads(info_path.read_text(encoding="utf-8"))
    lock = json.loads((ROOT / "scripts/native-lock.json").read_text(encoding="utf-8"))
    marker = "bili-replacement-smoke"
    if (info.get("target") != target or info.get("source") != lock
            or info.get("license") != "LGPL-2.1-or-later" or info.get("patches") != []):
        raise ValueError("replacement libraries do not match the supported target/source/license pin")
    if f"--extra-version={marker}" not in info.get("configure", []):
        raise ValueError("replacement libraries lack the required source-build marker")
    if sha256(ROOT / "scripts/build_ffmpeg.py") != info.get("recipe_sha256"):
        raise ValueError("replacement libraries were not built by the current recipe")
    for name in ("config.h", "config_components.h", "config.mak"):
        text = (prefix / name).read_text(encoding="utf-8", errors="replace")
        if has_forbidden_native_options(text):
            raise ValueError(f"replacement FFmpeg configuration enables forbidden option in {name}")
    before = {path.relative_to(root).as_posix(): sha256(path) for path in root.rglob("*") if path.is_file()}
    if target == "x86_64-unknown-linux-gnu":
        source_lib = prefix / "lib"
        dest_lib = root / "lib"
        sources = sorted(path for path in source_lib.glob("*.so*") if path.is_file() or path.is_symlink())
        package_names = {path.name for path in dest_lib.glob("*.so*")}
        if {path.name for path in sources} != package_names:
            raise ValueError("replacement FFmpeg library layout does not exactly match package lib/ closure")
        for source in sources:
            destination = dest_lib / source.name
            if destination.exists() or destination.is_symlink():
                destination.unlink()
            if source.is_symlink():
                destination.symlink_to(os.readlink(source))
            else:
                shutil.copy2(source, destination)
                run(["patchelf", "--set-rpath", "$ORIGIN", str(destination)], env, root)
        inspect_linux(root, env)
    else:
        source_dlls = [path for path in (prefix / "bin").rglob("*.dll") if path.is_file()]
        variants = ({"avformat-62.dll", "libavformat-62.dll"}, {"avcodec-62.dll", "libavcodec-62.dll"}, {"avutil-60.dll", "libavutil-60.dll"})
        selected = [path for path in source_dlls if any(path.name.lower() in group for group in variants)]
        if not all(any(path.name.lower() in group for path in selected) for group in variants):
            raise ValueError("replacement prefix lacks the three required FFmpeg DLLs")
        for source in selected:
            shutil.copy2(source, root / source.name)
        inspect_windows(root, env)
    allowed = {path.relative_to(root).as_posix() for path in (root / "lib").glob("*.so*")} if target == "x86_64-unknown-linux-gnu" else {path.name for path in root.glob("*.dll") if path.name.lower().startswith(("avformat", "libavformat", "avcodec", "libavcodec", "avutil", "libavutil"))}
    after = {path.relative_to(root).as_posix(): sha256(path) for path in root.rglob("*") if path.is_file()}
    changed = {name for name in before.keys() | after.keys() if before.get(name) != after.get(name)}
    if not changed or not changed.issubset(allowed):
        raise ValueError(f"replacement smoke changed files outside FFmpeg shared libraries: {sorted(changed - allowed)}")
    return marker

def json_lines(text):
    events = []
    for line in text.splitlines():
        if not line.strip():
            continue
        event = json.loads(line)
        if event.get("schema_version") != 1 or not isinstance(event.get("type"), str):
            raise ValueError("CLI emitted an invalid or unsupported JSON result envelope")
        events.append(event)
    return events

def committed_job_record(state_dir, job_id):
    job_id = str(uuid.UUID(str(job_id)))
    queue_path = state_dir / "queue.json"
    for _ in range(4):
        before = json.loads(queue_path.read_text(encoding="utf-8"))
        jobs = before.get("jobs")
        queue_revision = before.get("revision")
        if (before.get("schema_version") != 1 or not isinstance(queue_revision, int)
                or isinstance(queue_revision, bool) or queue_revision <= 0 or not isinstance(jobs, list)):
            raise ValueError("committed queue index has an invalid schema")
        matches = [entry for entry in jobs if isinstance(entry, dict) and entry.get("id") == job_id]
        if len(matches) != 1:
            raise ValueError("job is absent or duplicated in the committed queue index")
        revision = matches[0].get("revision")
        if not isinstance(revision, int) or isinstance(revision, bool) or revision <= 0:
            raise ValueError("committed queue index references an invalid job revision")
        checkpoint_path = state_dir / "jobs" / job_id / f"job-{revision}.json"
        if checkpoint_path.is_symlink() or not checkpoint_path.is_file():
            after = json.loads(queue_path.read_text(encoding="utf-8"))
            if before != after:
                continue
            raise ValueError("committed queue index references a missing or unsafe job checkpoint")
        record = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        after = json.loads(queue_path.read_text(encoding="utf-8"))
        if before != after:
            continue
        snapshot = record.get("snapshot")
        snapshot_revision = snapshot.get("revision") if isinstance(snapshot, dict) else None
        checkpoint_revision = record.get("checkpoint_revision")
        if (record.get("schema_version") != 1 or not isinstance(checkpoint_revision, int)
                or isinstance(checkpoint_revision, bool) or checkpoint_revision != revision
                or not isinstance(snapshot_revision, int) or isinstance(snapshot_revision, bool)
                or snapshot_revision < 0 or snapshot_revision > queue_revision
                or str(snapshot.get("id")) != job_id):
            raise ValueError("job checkpoint does not match its exact committed queue revision")
        return record
    raise ValueError("queue changed during committed job checkpoint inspection")

def validate_paused_resource_checkpoints(state_dir, job_id, record):
    resources = record.get("resources")
    if not isinstance(resources, list) or not resources:
        raise ValueError("managed checkpoint lacks resource identities")
    job_dir = state_dir / "jobs" / job_id
    durable_bytes = 0
    completed = 0
    checkpoint_count = 0
    checkpoint_items = []
    for resource in resources:
        name = resource.get("name") if isinstance(resource, dict) else None
        identity = resource.get("identity") if isinstance(resource, dict) else None
        if (not isinstance(name, str) or not name or name in {".", ".."}
                or Path(name).name != name or "/" in name or "\\" in name
                or not isinstance(identity, str) or not identity):
            raise ValueError("managed checkpoint has an invalid resource identity")
        media = job_dir / name
        sidecar = media.with_name(media.name + ".checkpoint.json")
        if not media.exists() and not sidecar.exists():
            checkpoint_items.append({
                "resource": name, "checkpoint_file": sidecar.name,
                "durable_bytes": 0, "complete": False, "sha256": None, "state": "not_started",
            })
            continue
        if media.is_symlink() or not media.is_file() or sidecar.is_symlink() or not sidecar.is_file():
            raise ValueError("durable managed resource or byte-offset sidecar is missing or unsafe")
        checkpoint = json.loads(sidecar.read_text(encoding="utf-8"))
        checkpoint_count += 1
        offset = checkpoint.get("durable_bytes")
        expected = checkpoint.get("expected_total")
        complete = checkpoint.get("complete")
        if (not isinstance(offset, int) or isinstance(offset, bool) or offset < 0
                or (expected is not None and (not isinstance(expected, int) or isinstance(expected, bool) or expected < 0))
                or not isinstance(complete, bool) or media.stat().st_size != offset):
            raise ValueError("resource byte-offset checkpoint does not match its durable file")
        digest = checkpoint.get("sha256")
        if offset:
            if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest) or sha256(media) != digest:
                raise ValueError("resource checkpoint digest does not match durable bytes")
        if expected is not None and offset > expected:
            raise ValueError("resource byte offset exceeds its declared total")
        if complete:
            if expected != offset or not offset:
                raise ValueError("completed resource checkpoint has an invalid total/offset")
            completed += 1
        checkpoint_items.append({
            "resource": name, "checkpoint_file": sidecar.name,
            "durable_bytes": offset, "complete": complete, "sha256": digest,
            "state": "complete" if complete else "paused",
        })
        durable_bytes += offset
    if durable_bytes <= 0:
        raise ValueError("paused job has no durable resource offset")
    return {
        "resource_count": len(resources), "durable_bytes": durable_bytes,
        "completed_count": completed, "checkpoint_count": checkpoint_count,
        "items": checkpoint_items,
    }


def validate_doctor(output, version_marker=None):
    events = json_lines(output)
    event = next((item for item in events if item.get("type") == "doctor"), None)
    if not event:
        raise ValueError("doctor did not emit its schema-versioned result")
    native = event.get("data", {}).get("native", {})
    libraries = native.get("libraries", [])
    names = {entry[0] for entry in libraries if isinstance(entry, list) and len(entry) == 2}
    if not {"avformat", "avcodec", "avutil"}.issubset(names):
        raise ValueError("doctor did not prove the expected in-process FFmpeg libraries are loaded")
    if version_marker and version_marker not in native.get("ffmpeg_version", ""):
        raise ValueError("doctor did not report the source-built replacement FFmpeg version marker")
    return {
        "ffmpeg_version": native.get("ffmpeg_version"),
        "libraries": [entry for entry in libraries if isinstance(entry, list) and len(entry) == 2],
    }


def validate_local_merge(stdout, state_dir, fixture, output):
    jobs = result_jobs(stdout)
    if len(jobs) != 1:
        raise ValueError("replacement fixture merge did not return exactly one managed result")
    job = jobs[0]
    if job.get("phase") != "Completed" or job.get("error") is not None or any(c.get("error") is not None for c in job.get("components", [])):
        raise ValueError("replacement fixture merge did not complete without component errors")
    try:
        job_id = str(uuid.UUID(job["id"]))
    except (KeyError, ValueError, TypeError) as error:
        raise ValueError("replacement fixture merge returned an invalid job ID") from error
    record = committed_job_record(state_dir, job_id)
    local = record.get("local") or {}
    completion = record.get("completion") or {}
    if (record.get("schema_version") != 1 or record.get("snapshot", {}).get("phase") != "Completed"
            or Path(local.get("video", "")).resolve() != fixture.resolve()
            or Path(local.get("output", "")).resolve() != output.resolve()
            or completion.get("fingerprint") != f"manual:{job_id}"
            or completion.get("promoted") is not True):
        raise ValueError("replacement fixture remux lacks its durable managed completion proof")
    if (job.get("output") != str(output) or not output.is_file() or output.is_symlink()
            or completion.get("size") != output.stat().st_size
            or completion.get("sha256") != sha256(output) or output.stat().st_size == 0):
        raise ValueError("replacement fixture remux output/hash/size proof is invalid")


def verify_replacement_remux(executable, state_dir, fixture, output, probe_helper, env, cwd):
    manifest_path = fixture.parent / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    entry = next((item for item in manifest.get("fixtures", []) if item.get("path") == fixture.name), None)
    if manifest.get("schema_version") != 1 or manifest.get("synthetic_only") is not True or not entry or entry.get("sha256") != sha256(fixture):
        raise ValueError("replacement remux input is not a verified synthetic fixture")
    state_dir.mkdir(mode=0o700)
    command = [str(executable), "--state-dir", str(state_dir), "--json", "merge",
               "--video", str(fixture), "--output", str(output)]
    result = subprocess.run(command, cwd=cwd, env=env, text=True, encoding="utf-8", capture_output=True)
    if result.returncode:
        raise RuntimeError(f"replacement-library managed remux failed ({result.returncode}): {result.stderr.strip()}\n{result.stdout.strip()}")
    validate_local_merge(result.stdout, state_dir, fixture, output)
    probe = json.loads(run([str(probe_helper), "--probe", str(output)], env, cwd, encoding="utf-8"))
    streams = probe.get("streams") if isinstance(probe, dict) else None
    kinds = {str(stream.get("media_type", "")).lower() for stream in streams or [] if isinstance(stream, dict)}
    if not {"video", "audio"}.issubset(kinds):
        raise ValueError("matched core SDK probe did not find both remuxed fixture streams")


def danmaku_output_count(name, path):
    text = path.read_text(encoding="utf-8")
    if name == "danmaku-raw":
        rows = [json.loads(line) for line in text.splitlines()]
        if any(not isinstance(row, dict)
               or not isinstance(row.get("id"), str)
               or not isinstance(row.get("text"), str)
               or type(row.get("time_ms")) is not int or row["time_ms"] < 0
               or type(row.get("mode")) is not int or row["mode"] < 0 for row in rows):
            raise ValueError("malformed danmaku JSONL comment")
        return len(rows)
    if name == "danmaku-xml":
        root = ET.fromstring(text)
        metadata = {"chatserver", "chatid", "mission", "maxlimit", "state", "real_name", "source"}
        if (root.tag != "i" or not metadata.issubset({child.tag for child in root})
                or any(child.tag not in metadata | {"d"} for child in root)):
            raise ValueError("malformed danmaku XML document")
        comments = root.findall("d")
        if any(len(comment.get("p", "").split(",")) != 8 or list(comment) for comment in comments):
            raise ValueError("malformed danmaku XML comment")
        return len(comments)
    section = None
    sections = {}
    count = 0
    expected_format = ["Layer", "Start", "End", "Style", "Name", "MarginL", "MarginR", "MarginV", "Effect", "Text"]
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith(";"):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            sections.setdefault(section, {})
            continue
        if section is None or ":" not in line:
            raise ValueError("malformed danmaku ASS document")
        key, value = (part.strip() for part in line.split(":", 1))
        sections[section][key] = value
        if section == "Events" and key == "Dialogue":
            fields = value.split(",", 9)
            if (len(fields) != 10
                    or any(not re.fullmatch(r"\d+:\d{2}:\d{2}\.\d{2}", fields[index]) for index in (1, 2))):
                raise ValueError("malformed danmaku ASS dialogue")
            count += 1
        elif section == "Events" and key != "Format":
            raise ValueError("unexpected danmaku ASS event")
    if (sections.get("Script Info", {}).get("ScriptType") != "v4.00+"
            or not sections.get("V4+ Styles", {}).get("Style")
            or [part.strip() for part in sections.get("Events", {}).get("Format", "").split(",")] != expected_format):
        raise ValueError("malformed danmaku ASS headers")
    return count


def validate_sidecar_outputs(components, output_dir):
    if not isinstance(components, list):
        raise ValueError("managed job contains a malformed sidecar result list")
    outcomes = []
    danmaku = {}
    for component in components:
        if not isinstance(component, dict) or not isinstance(component.get("name"), str) or not component["name"]:
            raise ValueError("managed job contains a malformed sidecar result")
        omissions = component.get("omissions", 0)
        if not isinstance(omissions, int) or isinstance(omissions, bool) or omissions < 0:
            raise ValueError("sidecar result contains an invalid omission count")
        item = {"name": component["name"], "omissions": omissions}
        if component.get("error") is not None:
            item["outcome"] = "failed"
        elif component.get("path") is None:
            item["outcome"] = "not-produced"
        else:
            candidate = Path(component["path"])
            if not candidate.is_absolute():
                candidate = output_dir / candidate
            if candidate.is_symlink() or not candidate.is_file():
                raise ValueError("managed sidecar path is missing or unsafe")
            path = candidate.resolve()
            if not path.is_relative_to(output_dir.resolve()):
                raise ValueError("managed sidecar is outside the isolated output directory")
            if component["name"] in {"danmaku-raw", "danmaku-xml", "danmaku-ass"}:
                if component["name"] in danmaku:
                    raise ValueError("duplicate managed danmaku result")
                count = danmaku_output_count(component["name"], path)
                danmaku[component["name"]] = (count, omissions)
                item["count"] = count
            elif path.stat().st_size <= 0:
                raise ValueError("managed sidecar is empty")
            item.update({
                "outcome": "produced",
                "path": path.name,
                "size": path.stat().st_size,
                "sha256": sha256(path),
            })
        outcomes.append(item)
    if danmaku:
        if set(danmaku) != {"danmaku-raw", "danmaku-xml", "danmaku-ass"}:
            raise ValueError("managed danmaku formats are incomplete")
        raw, xml, ass = (danmaku[name] for name in ("danmaku-raw", "danmaku-xml", "danmaku-ass"))
        if raw != xml or ass[0] + ass[1] != raw[0] + raw[1]:
            raise ValueError("managed danmaku counts/omissions disagree")
    return outcomes


def validate_managed_job(job, state_dir, output_dir):
    if job.get("phase") != "Completed" or job.get("error") is not None:
        raise ValueError("public-media job is not Completed or contains an error")
    components = job.get("components", [])
    sidecar_outputs = validate_sidecar_outputs(components, output_dir)
    if any(component.get("error") is not None for component in components):
        raise ValueError("a requested media component failed")
    plan = job.get("plan") or {}
    fingerprint = plan.get("fingerprint")
    if not fingerprint or plan.get("schema_version") != 1:
        raise ValueError("completed result lacks its managed plan identity")
    try:
        job_id = str(uuid.UUID(job["id"]))
    except (KeyError, ValueError, TypeError) as error:
        raise ValueError("CLI returned an invalid managed job ID") from error
    record = committed_job_record(state_dir, job_id)
    snapshot = record.get("snapshot", {})
    completion = record.get("completion") or {}
    if (record.get("checkpoint_revision", 0) < 1 or record.get("components_pending") is not False
            or snapshot.get("phase") != "Completed" or snapshot.get("plan", {}).get("fingerprint") != fingerprint
            or completion.get("fingerprint") != fingerprint or completion.get("promoted") is not True):
        raise ValueError("durable managed completion identity/promotion record is invalid")
    resources = record.get("resources")
    if (not isinstance(resources, list) or not resources
            or any(not isinstance(resource, dict) or not isinstance(resource.get("name"), str)
                   or Path(resource["name"]).name != resource["name"] or "/" in resource["name"]
                   or "\\" in resource["name"] or not isinstance(resource.get("identity"), str)
                   or not resource["identity"] for resource in resources)):
        raise ValueError("durable managed completion lacks safe resource identities")
    resource_summary = {"resource_count": len(resources), "keep_temp": plan.get("keep_temp")}
    output_text = job.get("output")
    if not output_text or output_text != snapshot.get("output"):
        raise ValueError("completed result and durable record disagree on output path")
    output = Path(output_text).resolve()
    if not output.is_relative_to(output_dir.resolve()) or not output.is_file() or output.is_symlink():
        raise ValueError("managed output is missing, unsafe, or outside the isolated output directory")
    digest = sha256(output)
    if (not re.fullmatch(r"[0-9a-f]{64}", completion.get("sha256", ""))
            or digest != completion["sha256"] or completion.get("size") != output.stat().st_size
            or output.stat().st_size <= 0):
        raise ValueError("managed completion hash/size does not match the promoted output")
    return job_id, output, {
        "sha256": completion["sha256"],
        "size": completion["size"],
        "resources": resource_summary,
        "sidecars": {
            "requested": plan.get("sidecars"),
            "components": sidecar_outputs,
        },
    }


def result_jobs(output, *, require_skipped=None):
    events = json_lines(output)
    if any(item.get("type") == "error" for item in events):
        raise ValueError("CLI reported an error during package media smoke")
    results = [item.get("data", {}) for item in events if item.get("type") == "result"]
    if not results:
        raise ValueError("CLI emitted no final result records")
    jobs = []
    for result in results:
        job = result.get("job")
        if not isinstance(job, dict) or result.get("exit_code") != 0:
            raise ValueError("CLI result contains a failed or malformed job")
        if require_skipped is not None and job.get("skipped") is not require_skipped:
            raise ValueError("repeat-download did not report the expected verified skip")
        jobs.append(job)
    return jobs


def interruptible_download(command, env, cwd, state_dir, timeout=180):
    creationflags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, encoding="utf-8", bufsize=1, creationflags=creationflags)
    lines = queue.Queue()
    stderr = []
    stderr_size = 0
    stderr_truncated = False
    diagnostics = []
    event_count = 0
    max_stderr = 65536


    def redact(value):
        value = re.sub(r"(?im)^(\s*(?:set-cookie|cookie|authorization|proxy-authorization)\s*:\s*).*$",
                       r"\1[redacted]", value)
        value = re.sub(r"https?://[^\s\"'<>]+", "[redacted URL]", value, flags=re.IGNORECASE)
        return re.sub(
            r"(?i)\b(sessdata|bili_jct|dedeuserid|dedeuserid_ckmd5|buvid3|buvid4|bili_ticket|ac_time_value|csrf|cookie|token|secret|access[_-]?key|refresh[_-]?token)\b\s*[:=]\s*([^\s,;]+)",
            r"\1=[redacted]", value,
        )
    def error_fields(error):
        if not isinstance(error, dict):
            return None
        fields = {}
        for key in ("code", "operation", "retryable", "native_code"):
            value = error.get(key)
            if isinstance(value, str):
                fields[key] = redact(value)[:256]
            elif isinstance(value, (bool, int)) or value is None:
                if value is not None:
                    fields[key] = value
        status = error.get("http_status")
        if isinstance(status, int) and not isinstance(status, bool) and 100 <= status <= 599:
            fields["http_status"] = status
        return fields

    def job_fields(job):
        if not isinstance(job, dict):
            return None
        result = {
            key: job[key] for key in ("id", "phase", "downloaded", "total", "skipped")
            if key in job and (isinstance(job.get(key), (str, int, bool)) or job.get(key) is None)
        }
        if isinstance(job.get("error"), dict):
            result["error"] = error_fields(job["error"])
        components = job.get("components")
        if isinstance(components, list):
            result["components"] = [
                {
                    "name": redact(item["name"])[:128],
                    **({"error": error_fields(item["error"])} if isinstance(item.get("error"), dict) else {}),
                    **({"omissions": item["omissions"]} if isinstance(item.get("omissions"), int) else {}),
                }
                for item in components if isinstance(item, dict) and isinstance(item.get("name"), str)
            ][:16]
        return result

    def remember_event(line):
        nonlocal event_count
        event_count += 1
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            event = None
            summary = {"type": "invalid-json"}
        else:
            kind = event.get("type") if isinstance(event, dict) else None
            if (not isinstance(event, dict) or event.get("schema_version") != 1 or not isinstance(kind, str)):
                summary = {
                    "type": "invalid-envelope",
                    "schema_version": event.get("schema_version") if isinstance(event, dict) else None,
                    "event_type": redact(kind)[:128] if isinstance(kind, str) else None,
                }
                event = None
            elif kind == "job":
                summary = {"type": kind, "job": job_fields(event.get("data"))}
            elif kind == "error":
                data = event.get("data", {})
                summary = {
                    "type": kind,
                    "error": error_fields(data.get("error")) if isinstance(data, dict) else None,
                    "exit_code": data.get("exit_code") if isinstance(data, dict) else None,
                }
            elif kind == "result":
                data = event.get("data", {})
                summary = {
                    "type": kind,
                    "job": job_fields(data.get("job")) if isinstance(data, dict) else None,
                    "exit_code": data.get("exit_code") if isinstance(data, dict) else None,
                }
            else:
                summary = {"type": redact(kind)[:128]}
        diagnostics.append(summary)
        if len(diagnostics) > 12:
            del diagnostics[:-12]
        return event

    def read_stdout():
        for line in process.stdout:
            lines.put(line)

    def read_stderr():
        nonlocal stderr_size, stderr_truncated
        for line in process.stderr:
            if len(line) > max_stderr:
                line = line[-max_stderr:]
                stderr_truncated = True
            stderr.append(line)
            stderr_size += len(line)
            while stderr_size > max_stderr and stderr:
                stderr_size -= len(stderr.pop(0))
                stderr_truncated = True

    def finish_readers():
        out_thread.join()
        err_thread.join()
        while True:
            try:
                remember_event(lines.get_nowait())
            except queue.Empty:
                break

    def safe_stderr():
        text = redact("".join(stderr))
        if stderr_truncated:
            text = "[earlier stderr omitted]\n" + text
        return text[-8192:].strip() or "<empty>"

    def failure_details(reason, return_code):
        return (f"{reason}; CLI return code {return_code}; recent safe CLI events "
                f"({min(event_count, len(diagnostics))} of {event_count}): "
                f"{json.dumps(diagnostics, sort_keys=True, separators=(',', ':'))}; "
                f"safe stderr: {safe_stderr()}")

    out_thread = threading.Thread(target=read_stdout, daemon=True)
    err_thread = threading.Thread(target=read_stderr, daemon=True)
    out_thread.start()
    err_thread.start()
    job_id = None
    interrupted = False
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=0.15)
        except queue.Empty:
            if process.poll() is not None:
                break
            continue
        event = remember_event(line)
        if not isinstance(event, dict) or event.get("schema_version") != 1 or event.get("type") != "job":
            continue
        job = event.get("data", {})
        downloaded = job.get("downloaded") if isinstance(job, dict) else None
        if (isinstance(job, dict) and job.get("phase") == "Downloading"
                and isinstance(downloaded, int) and not isinstance(downloaded, bool) and downloaded > 0):
            job_id = job.get("id")
            try:
                if os.name == "nt":
                    process.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    process.send_signal(signal.SIGINT)
            except OSError as error:
                try:
                    process.kill()
                except OSError:
                    pass
                return_code = process.wait()
                finish_readers()
                raise RuntimeError(failure_details("CLI exited or rejected the interrupt signal", return_code)) from error
            interrupted = True
            break
    if not interrupted:
        observed_return_code = process.poll()
        timed_out = observed_return_code is None and time.monotonic() >= deadline
        if observed_return_code is None:
            try:
                process.kill()
            except OSError:
                pass
            return_code = process.wait()
        else:
            return_code = observed_return_code
        finish_readers()
        reason = (f"public media preflight timed out after {timeout}s before positive-byte Downloading progress"
                  if timed_out else "CLI exited before positive-byte Downloading progress")
        raise RuntimeError(failure_details(reason, return_code))
    try:
        process.wait(timeout=120)
    except subprocess.TimeoutExpired:
        try:
            process.kill()
        except OSError:
            pass
        return_code = process.wait()
        finish_readers()
        raise RuntimeError(failure_details("CLI did not finish graceful interrupt/checkpoint shutdown", return_code))
    finish_readers()
    if process.returncode != 130:
        raise RuntimeError(failure_details(
            f"interrupted download returned {process.returncode}, expected Ctrl+C exit 130",
            process.returncode,
        ))
    if not job_id:
        raise RuntimeError("interrupt checkpoint was observed without a managed job ID")
    try:
        job_id = str(uuid.UUID(job_id))
    except ValueError as error:
        raise RuntimeError("interrupt stream included an invalid job ID") from error
    checkpoint = committed_job_record(state_dir, job_id)
    snapshot = checkpoint.get("snapshot", {})
    if snapshot.get("phase") != "Paused" or checkpoint.get("completion") is not None:
        raise RuntimeError("Ctrl+C did not leave a resumable incomplete managed checkpoint")
    resource_summary = validate_paused_resource_checkpoints(state_dir, job_id, checkpoint)
    reported_downloaded = snapshot.get("downloaded")
    if not isinstance(reported_downloaded, int) or isinstance(reported_downloaded, bool) or reported_downloaded <= 0:
        raise RuntimeError("paused managed checkpoint has no recorded transfer progress")
    return job_id, {
        "checkpoint_revision": checkpoint["checkpoint_revision"],
        "reported_downloaded_bytes": reported_downloaded,
        "resources": resource_summary,
    }


def find_probe_helper(sdk_archive, extract_dir):
    extract_dir.mkdir(mode=0o700)
    extract(sdk_archive, extract_dir)
    sdk = package_root(extract_dir)
    info_path = sdk / "build-info.json"
    info = json.loads(info_path.read_text(encoding="utf-8"))
    if info.get("mode") != "dylib":
        raise ValueError("live smoke probe requires the matched dylib SDK artifact")
    executable = (sdk / info.get("consumer_executable", "")).resolve()
    if not executable.is_relative_to(sdk.resolve()) or not executable.is_file():
        raise ValueError("dylib SDK metadata does not name an in-package probe consumer")
    return executable


def validate_typed_result(output, expected_type):
    events = json_lines(output)
    if any(event.get("type") == "error" for event in events):
        raise ValueError(f"{expected_type} reported an error")
    results = [event for event in events if event.get("type") == expected_type]
    if len(events) != 1 or len(results) != 1 or results[0].get("schema_version") != 1:
        raise ValueError(f"{expected_type} did not emit exactly one schema-version-1 typed result")
    data = results[0].get("data")
    if not isinstance(data, dict):
        raise ValueError(f"{expected_type} result data is not an object")
    return data

def public_media_identifier(value):
    if not isinstance(value, str):
        return None
    match = re.search(r"(?<![A-Za-z0-9])(?:BV[0-9A-Za-z]+|av[0-9]+)(?![A-Za-z0-9])", value, re.IGNORECASE)
    if not match:
        return None
    identifier = match.group(0)
    return "BV" + identifier[2:] if identifier.lower().startswith("bv") else "av" + identifier[2:]


def verify_discovery(executable, state_dir, media_id, env, cwd, deadline=None):
    command = [str(executable), "--state-dir", str(state_dir), "--json"]
    info = validate_typed_result(run(command + ["info", media_id, "--auth", "anonymous"], env, cwd,
                                     encoding="utf-8", deadline=deadline), "info")
    parts = info.get("parts")
    if not isinstance(info.get("title"), str) or not info["title"].strip() or not isinstance(parts, list) or not parts:
        raise ValueError("info response lacks typed title/part discovery fields")
    part = next((entry for entry in parts if isinstance(entry, dict) and entry.get("ordinal") == 1), None)
    if not part or not isinstance(part.get("title"), str) or not part["title"].strip() or not isinstance(part.get("reference"), dict):
        raise ValueError("info response lacks the requested typed part reference")
    catalog = validate_typed_result(
        run(command + ["qualities", media_id, "--part", "1", "--auth", "anonymous"], env, cwd,
            encoding="utf-8", deadline=deadline), "qualities",
    )
    if (catalog.get("part") != part["reference"] or not isinstance(catalog.get("advertised"), list)
            or not isinstance(catalog.get("representations"), list)
            or not isinstance(catalog.get("revision"), int) or catalog["revision"] < 0
            or not isinstance(catalog.get("preview"), bool)):
        raise ValueError("qualities response lacks a typed catalog for the discovered part")
    for quality in catalog["advertised"]:
        if (not isinstance(quality, dict) or not isinstance(quality.get("id"), int) or quality["id"] <= 0
                or not isinstance(quality.get("label"), str) or not quality["label"]
                or quality.get("availability") not in {"Obtainable", "LoginRequired", "EntitlementRequired", "RegionRestricted", "PreviewOnly", "Unknown", "Unavailable"}):
            raise ValueError("qualities response contains a malformed advertised quality")
    for representation in catalog["representations"]:
        if (not isinstance(representation, dict) or representation.get("part") != catalog["part"]
                or not isinstance(representation.get("id"), int) or representation["id"] <= 0
                or not isinstance(representation.get("quality"), int) or representation["quality"] <= 0
                or not isinstance(representation.get("codec"), str) or not isinstance(representation.get("audio"), bool)):
            raise ValueError("qualities response contains a malformed representation")

    if not catalog["representations"]:
        raise ValueError("qualities response contains no selectable stream representations")
    resolved_id = public_media_identifier(info.get("bvid")) or public_media_identifier(media_id)
    media_path = f"https://www.bilibili.com/video/{resolved_id}" if resolved_id else None
    return {
        "media_id": resolved_id,
        "media_path": media_path,
        "title": info["title"],
        "part_title": part["title"],
        "part_reference": part["reference"],
        "catalog_revision": catalog["revision"],
        "advertised_quality_count": len(catalog["advertised"]),
        "representation_count": len(catalog["representations"]),
        "catalog": catalog,
    }


def canonical_codec(codec):
    if not isinstance(codec, str) or not codec.strip():
        return None
    token = codec.strip().lower().split(".", 1)[0]
    return {
        "avc": "h264", "avc1": "h264", "avc3": "h264",
        "hev1": "hevc", "hvc1": "hevc", "av01": "av1",
        "mp4a": "aac", "ac-3": "ac3", "ec-3": "eac3",
    }.get(token, token)


def validate_selected_media(probe, plan):
    streams = probe.get("streams") if isinstance(probe, dict) else None
    if not isinstance(streams, list):
        raise ValueError("matched core SDK probe returned no stream inventory")

    def require_representation(representation, media_type):
        if not isinstance(representation, dict):
            raise ValueError(f"selected {media_type} representation is missing")
        expected_codec = canonical_codec(representation.get("codec"))
        if not expected_codec or expected_codec == "unknown":
            raise ValueError(f"selected {media_type} representation has no verifiable codec")
        candidates = [
            stream for stream in streams
            if isinstance(stream, dict)
            and str(stream.get("media_type", "")).lower() == media_type
            and canonical_codec(stream.get("codec")) == expected_codec
        ]
        width = representation.get("width")
        height = representation.get("height")
        sample_rate = representation.get("sample_rate")
        channels = representation.get("channels")
        if (media_type == "video"
                and any(not isinstance(value, int) or isinstance(value, bool) or value <= 0 for value in (width, height))):
            raise ValueError("selected video representation lacks known positive dimensions")
        matches = [
            stream for stream in candidates
            if (media_type != "video"
                or (stream.get("width") == width and stream.get("height") == height))
            and (media_type != "audio"
                 or ((sample_rate is None or stream.get("sample_rate") == sample_rate)
                     and (channels is None or stream.get("channels") == channels)))
        ]
        if not matches:
            raise ValueError(f"completed media does not match selected {media_type} codec/dimensions")
        stream = matches[0]
        return {
            "expected": {
                "codec": representation.get("codec"), "width": width, "height": height,
                "sample_rate": sample_rate, "channels": channels,
            },
            "actual": {
                "codec": stream.get("codec"), "width": stream.get("width"), "height": stream.get("height"),
                "sample_rate": stream.get("sample_rate"), "channels": stream.get("channels"),
            },
        }

    if not isinstance(plan, dict) or not isinstance(plan.get("video"), dict):
        raise ValueError("completed public media result lacks its selected video plan")
    result = {"video": require_representation(plan["video"], "video")}
    if plan.get("audio") is not None:
        result["audio"] = require_representation(plan["audio"], "audio")
    return result


def verify_basic_live_media(executable, state_dir, output_dir, env, cwd, media_id,
                           probe_helper, report, timeout=900, max_bytes=134217728):
    if public_media_identifier(media_id) != "BV1gD4y1o72p":
        raise ValueError("basic profile requires the fixed authorized sample BV1gD4y1o72p")
    state_dir.mkdir(mode=0o700)
    output_dir.mkdir(mode=0o700)
    deadline = time.monotonic() + timeout
    discovery = verify_discovery(executable, state_dir, media_id, env, cwd, deadline)
    report["public_media_id"] = discovery["media_id"]
    report["discovery"] = {"outcome": "passed", **{k: v for k, v in discovery.items() if k != "catalog"}}
    base = [str(executable), "--state-dir", str(state_dir), "--json"]
    reps = discovery["catalog"]["representations"]
    videos = [item for item in reps if item["audio"] is False]
    audios = [item for item in reps if item["audio"] is True]
    if not videos or not audios:
        raise ValueError("basic requires returned video and audio representations")
    video = min(videos, key=lambda item: item["bandwidth"])
    audio = min(audios, key=lambda item: item["bandwidth"])
    estimated = (video["bandwidth"] + audio["bandwidth"]) * 635 // 8
    if estimated > max_bytes * 3 // 4:
        raise ValueError("lowest video/audio pair exceeds the conservative sample budget")
    command = base + ["download", media_id, "--parts", "1", "--output", str(output_dir),
                      "--auth", "anonymous", "--merge", "auto",
                      "--video-repr", str(video["id"]), "--audio-repr", str(audio["id"])]
    creationflags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    with tempfile.TemporaryFile(mode="w+b") as stdout, tempfile.TemporaryFile(mode="w+b") as stderr:
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                                   creationflags=creationflags)
        try:
            while process.poll() is None:
                if time.monotonic() >= deadline:
                    process.kill()
                    process.wait()
                    raise TimeoutError("basic deadline expired during download/merge")
                if os.fstat(stdout.fileno()).st_size > 16 * 1024 * 1024 or os.fstat(stderr.fileno()).st_size > 65536:
                    process.kill()
                    process.wait()
                    raise ValueError("basic CLI output exceeded its bound")
                resources = list(state_dir.glob("**/resource-*.media"))
                resource_bytes = sum(path.stat().st_size for path in resources if path.is_file())
                outputs = [path for path in output_dir.rglob("*") if path.is_file()]
                sample_bytes = resource_bytes + sum(path.stat().st_size for path in outputs)
                if sample_bytes > max_bytes:
                    process.kill()
                    process.wait()
                    raise ValueError("basic resource/output bytes exceeded its bound")
                time.sleep(0.1)
            if process.returncode:
                raise RuntimeError(f"basic CLI failed ({process.returncode})")
            if os.fstat(stdout.fileno()).st_size > 16 * 1024 * 1024 or os.fstat(stderr.fileno()).st_size > 65536:
                raise ValueError("basic CLI output exceeded its bound")
            stdout.seek(0)
            result = stdout.read(16 * 1024 * 1024 + 1)
            if len(result) > 16 * 1024 * 1024:
                raise ValueError("basic CLI output exceeded its bound")
            result = result.decode("utf-8")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    jobs = result_jobs(result)
    if len(jobs) != 1:
        raise ValueError("basic did not return exactly one managed job")
    job = jobs[0]
    if job.get("phase") != "Completed" or job.get("error") is not None or any(
            component.get("error") is not None for component in job.get("components", [])):
        raise ValueError("basic job or requested component failed")
    job_id, media, completion = validate_managed_job(job, state_dir, output_dir)
    plan = job.get("plan")
    if (not isinstance(plan, dict) or plan.get("video", {}).get("id") != video["id"]
            or plan.get("audio", {}).get("id") != audio["id"]):
        raise ValueError("completed job plan differs from the requested representations")
    resources = list(state_dir.glob("**/resource-*.media"))
    resource_bytes = sum(path.stat().st_size for path in resources if path.is_file())
    outputs = [path for path in output_dir.rglob("*") if path.is_file()]
    if resource_bytes + sum(path.stat().st_size for path in outputs) > max_bytes:
        raise ValueError("basic final resource/output bytes exceeded its bound")
    if not media.is_file() or media.stat().st_size <= 0 or media.stat().st_size > max_bytes:
        raise ValueError("basic output is empty or exceeds the byte budget")
    if completion.get("size") != media.stat().st_size or completion.get("sha256") != sha256(media):
        raise ValueError("basic output does not match its committed hash/size")
    if time.monotonic() >= deadline:
        raise TimeoutError("basic verification exceeded its deadline")
    streams = json.loads(run([str(probe_helper), "--probe", str(media)], env, cwd,
                             encoding="utf-8", deadline=deadline))
    selected = validate_selected_media(streams, plan)
    report["basic_smoke"] = {
        "outcome": "passed", "job_id": job_id, "phase": "Completed",
        "sha256": completion["sha256"], "size": completion["size"],
        "selected_streams": selected,
    }


def verify_live_media(executable, state_dir, output_dir, env, cwd, media_id, probe_helper, report):
    state_dir.mkdir(mode=0o700)
    output_dir.mkdir(mode=0o700)
    try:
        discovery = verify_discovery(executable, state_dir, media_id, env, cwd)
    except Exception as error:
        report["discovery"] = {"outcome": "failed", "error_type": type(error).__name__}
        raise
    report["public_media_id"] = discovery["media_id"]
    report["media_path"] = discovery["media_path"]
    report["discovery"] = {"outcome": "passed", **discovery}
    base = [str(executable), "--state-dir", str(state_dir), "--json"]
    initial = base + ["download", media_id, "--parts", "1", "--output", str(output_dir),
                      "--auth", "anonymous", "--merge", "auto"]
    try:
        job_id, pause = interruptible_download(initial, env, cwd, state_dir)
    except RuntimeError as error:
        report["pause"] = {"outcome": "failed", "diagnostic": str(error)}
        raise
    report["pause"] = {"outcome": "passed", "job_id": job_id, **pause}
    resume = subprocess.run(base + ["resume", job_id, "--auth", "anonymous"], cwd=cwd, env=env,
                            text=True, encoding="utf-8", capture_output=True)
    if resume.returncode:
        raise RuntimeError(f"actual resume smoke failed ({resume.returncode}): {resume.stderr.strip()}\n{resume.stdout.strip()}")
    resumed_jobs = result_jobs(resume.stdout)
    resumed = [job for job in resumed_jobs if job.get("id") == job_id]
    if len(resumed) != 1:
        raise ValueError("resume did not complete the interrupted managed job")
    resumed_id, media, completion = validate_managed_job(resumed[0], state_dir, output_dir)
    if resumed_id != job_id:
        raise ValueError("resume changed the managed job identity")
    report["resume"] = {"outcome": "passed", "job_id": job_id, "phase": "Completed", **completion}
    report["hash"] = {
        "outcome": "passed",
        "path": media.name,
        "sha256": completion["sha256"],
        "size": completion["size"],
        "verified_against_durable_completion": True,
    }
    probe = run([str(probe_helper), "--probe", str(media)], env, cwd, encoding="utf-8")
    probed = json.loads(probe)
    selected_streams = validate_selected_media(probed, resumed[0].get("plan"))
    stream_inventory = [
        {
            key: stream.get(key)
            for key in ("media_type", "codec", "width", "height", "sample_rate", "channels")
        }
        for stream in probed["streams"] if isinstance(stream, dict)
    ]
    report.setdefault("native", {}).update({
        "outcome": "passed",
        "matched_sdk_probe": str(probe_helper.name),
        "selected_streams": selected_streams,
        "stream_inventory": stream_inventory,
    })
    report["sidecars"] = {
        "outcome": "verified",
        "requested": completion["sidecars"]["requested"],
        "resource_checkpoints": pause["resources"]["items"],
        "components": completion["sidecars"]["components"],
    }
    repeated = subprocess.run(initial, cwd=cwd, env=env, text=True, encoding="utf-8", capture_output=True)
    if repeated.returncode:
        raise RuntimeError(f"verified-skip smoke failed ({repeated.returncode}): {repeated.stderr.strip()}\n{repeated.stdout.strip()}")
    skipped_jobs = result_jobs(repeated.stdout, require_skipped=True)
    if len(skipped_jobs) != 1 or skipped_jobs[0].get("id") != job_id:
        raise ValueError("repeat download did not return one verified skip for the resumed managed job")
    _, skipped_output, skipped_completion = validate_managed_job(skipped_jobs[0], state_dir, output_dir)
    if skipped_output != media or skipped_completion["sha256"] != completion["sha256"]:
        raise ValueError("verified skip did not preserve the resumed output hash")
    report["skip"] = {
        "outcome": "passed",
        "job_id": job_id,
        "result_count": len(skipped_jobs),
        "skipped": skipped_jobs[0].get("skipped") is True,
        "verified_output_sha256": skipped_completion["sha256"],
        "same_as_resumed_output": True,
    }
    return discovery


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--target", required=True, choices=("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"))
    parser.add_argument("--native-source", type=Path, required=True, help="matching separately packaged native-source archive")
    parser.add_argument("--probe-helper", type=Path, help="matched bili_core SDK probe consumer executable")
    parser.add_argument("--sdk-artifact", type=Path, help="matched dylib SDK tarball; safely extracted to provide the probe consumer")
    parser.add_argument("--replacement-prefix", type=Path, help="compatible source-built FFmpeg library prefix used for the replacement smoke")
    parser.add_argument("--replacement-fixture", type=Path, help="verified synthetic h264-aac.mp4 input for replacement-library managed remux")
    parser.add_argument("--offline-fixture", type=Path, help="synthetic fixture for an isolated managed remux smoke")
    parser.add_argument("--public-media-id", help="opt in to anonymous live media smoke")
    parser.add_argument("--public-media-profile", choices=("full-recovery", "basic"), default="full-recovery")
    parser.add_argument("--live-timeout-seconds", type=int, default=900)
    parser.add_argument("--max-media-bytes", type=int, default=134217728)
    parser.add_argument("--report", type=Path, help="write an artifact-safe JSON record of actual verification outcomes")
    args = parser.parse_args()
    if args.live_timeout_seconds <= 0 or args.max_media_bytes <= 0:
        parser.error("--live-timeout-seconds and --max-media-bytes must be positive")
    if args.offline_fixture and not (args.probe_helper or args.sdk_artifact):
        parser.error("--offline-fixture requires --probe-helper or --sdk-artifact")
    if args.offline_fixture and not args.offline_fixture.is_file():
        parser.error("--offline-fixture does not exist")
    public_id = public_media_identifier(args.public_media_id)
    if args.public_media_profile == "basic":
        if not args.public_media_id:
            parser.error("basic requires --public-media-id BV1gD4y1o72p")
        if public_id != "BV1gD4y1o72p" or args.target != "x86_64-unknown-linux-gnu":
            parser.error("basic requires BV1gD4y1o72p on x86_64-unknown-linux-gnu")
        if not args.probe_helper and not args.sdk_artifact:
            parser.error("basic requires --probe-helper or --sdk-artifact")
    report = {
        "schema_version": 1,
        "status": "incomplete",
        "target": args.target,
        "public_media_profile": args.public_media_profile if args.public_media_id else None,
        "public_media_id": public_id,
        "media_path": f"https://www.bilibili.com/video/{public_id}" if public_id else None,
        "public_media_input_provided": bool(args.public_media_id),
        "authentication": "anonymous" if args.public_media_id else None,
    }

    def write_report():
        if args.report is None:
            return
        args.report.parent.mkdir(parents=True, exist_ok=True)
        temporary_report = args.report.with_name(args.report.name + ".tmp")
        temporary_report.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary_report, args.report)

    atexit.register(write_report)
    report["package_sha256"] = sha256(args.package.resolve())
    report["native_source_sha256"] = sha256(args.native_source.resolve())
    with tempfile.TemporaryDirectory(prefix="bili-package-verify-") as temporary:
        base = Path(temporary).resolve()
        extracted = base / "extract"
        extracted.mkdir(mode=0o700)
        extract(args.package.resolve(), extracted)
        root = package_root(extracted)
        check_checksums(root)
        source_asset = args.native_source.resolve()
        source_extract = base / "native-source"
        native_info = inspect_native(root, args.target, source_asset, source_extract)
        build_info = json.loads((root / "BUILD_INFO.json").read_text(encoding="utf-8"))
        validate_dependency_licenses(root, args.target)
        validate_windows_runtime(root, build_info, args.target)
        if build_info.get("target") != args.target or build_info.get("profile") not in ("debug", "release"):
            raise ValueError("application build tuple target/profile is invalid")
        binary = root / ("bili.exe" if args.target == "x86_64-pc-windows-msvc" else "bin/bili")
        if sha256(binary) != build_info.get("app_sha256"):
            raise ValueError("application binary checksum differs from build tuple")
        native_info_path = root / "share/bili/native/build-info.json"
        if build_info.get("native_build_info_sha256") != sha256(native_info_path):
            raise ValueError("native build metadata checksum differs from application build tuple")
        if build_info.get("cargo_lock_sha256") != sha256(ROOT / "Cargo.lock"):
            raise ValueError("package Cargo.lock tuple differs from checked source")
        files = [path for path in root.rglob("*") if path.is_file()]
        bad = [path for path in files if path.name.lower() in FORBIDDEN_NAMES or
               (path.suffix.lower() == ".exe" and path.name.lower() != "bili.exe") or
               path.suffix.lower() in (".pdb", ".ilk", ".exp")]
        if bad:
            raise ValueError(f"forbidden executable/debug artifact in package: {bad}")
        for path in files:
            with path.open("rb") as stream:
                magic = stream.read(4)
            if magic == bytes.fromhex("7f454c46") and path != binary and ".so" not in path.name:
                raise ValueError(f"unexpected ELF executable or non-library in package: {path}")
            if magic[:2] == b"MZ" and path.name.lower() != "bili.exe" and path.suffix.lower() != ".dll":
                raise ValueError(f"unexpected Windows executable in package: {path}")
        env = {
            "TEMP": str(base), "TMP": str(base), "TMPDIR": str(base), "HOME": str(base / "home"),
            "USERPROFILE": str(base / "home"), "APPDATA": str(base / "home/AppData/Roaming"),
            "LOCALAPPDATA": str(base / "home/AppData/Local"), "SYSTEMROOT": os.environ.get("SYSTEMROOT", ""),
            "WINDIR": os.environ.get("WINDIR", ""), "SystemRoot": os.environ.get("SystemRoot", ""),
        }
        inspect_env = env.copy()
        inspect_env["PATH"] = "/usr/bin:/bin" if args.target == "x86_64-unknown-linux-gnu" else os.pathsep.join(filter(None, (os.path.join(env["SYSTEMROOT"], "System32"),)))
        env["PATH"] = str(root / "bin") if args.target == "x86_64-unknown-linux-gnu" else os.pathsep.join((str(root), os.path.join(env["SYSTEMROOT"], "System32")))
        env["XDG_CONFIG_HOME"] = str(base / "config")
        env["XDG_DATA_HOME"] = str(base / "data")
        env["XDG_CACHE_HOME"] = str(base / "cache")
        for name in ("HOME", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            Path(env[name]).mkdir(mode=0o700, parents=True, exist_ok=True)
        if args.target == "x86_64-unknown-linux-gnu":
            inspect_linux(root, inspect_env)
            executable = root / "bin/bili"
        else:
            inspect_windows(root, inspect_env)
            executable = root / "bili.exe"
        run([str(executable), "--help"], env, base)
        run([str(executable), "--version"], env, base)
        doctor = run([str(executable), "--state-dir", str(base / "doctor-state"), "--json", "doctor"], env, base, encoding="utf-8")
        report["native"] = {"doctor": validate_doctor(doctor)}
        report["package_validation"] = "passed"
        probe_helper = args.probe_helper.resolve() if args.probe_helper else None
        if args.offline_fixture:
            if probe_helper is None:
                probe_helper = find_probe_helper(args.sdk_artifact.resolve(), base / "sdk")
            if not probe_helper.is_file():
                raise ValueError("matched SDK probe consumer is missing")
            verify_replacement_remux(executable, base / "offline-merge-state",
                                     args.offline_fixture.resolve(), base / "offline-merge.mp4",
                                     probe_helper, env, base)
            report["offline_merge_smoke"] = "passed"
        if args.replacement_prefix:
            if not args.replacement_fixture:
                raise ValueError("--replacement-prefix requires --replacement-fixture")
            if probe_helper is None:
                if not args.sdk_artifact:
                    raise ValueError("replacement-library smoke requires --probe-helper or --sdk-artifact")
                probe_helper = find_probe_helper(args.sdk_artifact.resolve(), base / "sdk")
            if not probe_helper.is_file():
                raise ValueError("matched SDK probe consumer is missing")
            marker = validate_replacement(args.replacement_prefix.resolve(), root, args.target, inspect_env)
            replacement_doctor = run([str(executable), "--state-dir", str(base / "replacement-doctor-state"),
                                      "--json", "doctor"], env, base, encoding="utf-8")
            report["native"]["replacement_doctor"] = validate_doctor(replacement_doctor, marker)
            verify_replacement_remux(executable, base / "replacement-state",
                                     args.replacement_fixture.resolve(), base / "replacement-remux.mp4",
                                     probe_helper, env, base)
            report["replacement_smoke"] = "passed"
        if args.public_media_id:
            if args.public_media_profile == "basic":
                if probe_helper is None:
                    probe_helper = find_probe_helper(args.sdk_artifact.resolve(), base / "sdk")
                if not probe_helper.is_file():
                    raise ValueError("matched SDK probe consumer is missing")
                verify_basic_live_media(executable, base / "state", base / "public-smoke", env, base,
                                        args.public_media_id, probe_helper, report,
                                        args.live_timeout_seconds, args.max_media_bytes)
            else:
                if probe_helper is None:
                    if not args.sdk_artifact:
                        raise ValueError("opt-in public-media smoke requires --probe-helper or --sdk-artifact for native output verification")
                    probe_helper = find_probe_helper(args.sdk_artifact.resolve(), base / "sdk")
                verify_live_media(executable, base / "state", base / "public-smoke", env, base,
                                  args.public_media_id, probe_helper, report)
        print("Package extraction, checksums, source/configuration, dependency closure, and isolated CLI smoke passed.")
        report["status"] = "passed"
        if args.replacement_prefix:
            print("Compatible-library replacement and synthetic managed-remux smoke passed.")
        if args.public_media_id:
            if args.public_media_profile == "basic":
                print("Opt-in anonymous basic download/merge smoke passed.")
            else:
                print("Opt-in anonymous full-recovery media smoke passed.")


if __name__ == "__main__":
    main()
