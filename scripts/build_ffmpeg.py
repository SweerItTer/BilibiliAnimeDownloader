#!/usr/bin/env python3
"""Build only pinned, replaceable FFmpeg shared libraries from upstream source."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--source-cache", type=Path, default=ROOT / "target/native/sources")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 1, 8))
    parser.add_argument("--extra-version", help="Public build marker for compatible-library replacement qualification")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if args.extra_version is not None and (not args.extra_version or
            any(not (c.isascii() and (c.isalnum() or c in "._-")) for c in args.extra_version)):
        parser.error("--extra-version must be a nonempty ASCII build marker")
    windows = args.target.endswith("windows-msvc")
    if windows != (sys.platform == "win32"):
        parser.error("build on the target operating system; this recipe is not a cross compiler")
    lock = json.loads((ROOT / "scripts/native-lock.json").read_text())
    prefix = (args.prefix or ROOT / "target/native" / args.target).resolve()
    cache = args.source_cache.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / lock["archive"]
    if not archive.exists():
        pending = archive.with_suffix(archive.suffix + ".partial")
        with urllib.request.urlopen(lock["url"], timeout=120) as source, pending.open("wb") as output:
            shutil.copyfileobj(source, output)
        if digest(pending) != lock["sha256"]:
            pending.unlink()
            raise SystemExit("FFmpeg source checksum mismatch")
        pending.replace(archive)
    if digest(archive) != lock["sha256"]:
        raise SystemExit("cached FFmpeg source checksum mismatch")
    work = prefix.parent / (prefix.name + "-build")
    source = work / ("ffmpeg-" + lock["version"])
    work.mkdir(parents=True, exist_ok=True)
    # Cargo cache cleanup may leave directory skeletons; restore every pinned member.
    with tarfile.open(archive) as bundle:
        bundle.extractall(work, filter="data")
    build = work / "objects"
    build.mkdir(exist_ok=True)
    flags = list(lock["configure"])
    if args.extra_version:
        flags.append("--extra-version=" + args.extra_version)
    # No decoder/encoder API is exposed to the app. Narrow probing decoders are
    # retained because libavformat may need them to identify legacy FLV streams.
    if windows:
        flags += ["--toolchain=msvc", "--arch=x86_64", "--target-os=win64", "--extra-cflags=-MD"]
    compiler = "cl" if windows else "cc"
    compiler_info = subprocess.run([compiler] if windows else [compiler, "--version"], capture_output=True, text=True)
    if windows:
        # MSYS paths for configure; toolchain remains inherited MSVC, not MinGW.
        bash = shutil.which("bash")
        if bash is None:
            raise SystemExit("MSYS bash is required for the Windows native build")
        def unix_path(path):
            return subprocess.check_output(["cygpath", "-u", str(path)], text=True).strip()
        command = [bash, "-c", "exec " + shlex.join([unix_path(source / "configure"), "--prefix=" + unix_path(prefix), *flags])]
    else:
        command = [str(source / "configure"), "--prefix=" + str(prefix), *flags]
    subprocess.run(command, cwd=build, check=True)
    subprocess.run(["make", "-j" + str(args.jobs)], cwd=build, check=True)
    subprocess.run(["make", "install"], cwd=build, check=True)
    prefix.mkdir(parents=True, exist_ok=True)
    import_libraries = []
    if windows:
        # FFmpeg's MSVC shared install puts genuine import libraries beside DLLs.
        (prefix / "lib").mkdir(exist_ok=True)
        for name in ("avformat.lib", "avcodec.lib", "avutil.lib"):
            installed_source = prefix / "bin" / name
            if not installed_source.is_file():
                raise SystemExit(f"expected source-built MSVC import library missing: {installed_source}")
            source_sha256 = digest(installed_source)
            installed_library = prefix / "lib" / name
            shutil.copy2(installed_source, installed_library)
            installed_sha256 = digest(installed_library)
            if installed_sha256 != source_sha256:
                raise SystemExit(f"MSVC import library copy checksum mismatch: {name}")
            record = {"source": f"bin/{name}", "installed": f"lib/{name}",
                      "source_sha256": source_sha256, "installed_sha256": installed_sha256}
            import_libraries.append(record)
            print("MSVC import library: " + json.dumps(record, sort_keys=True))
    for name in ("COPYING.LGPLv2.1", "LICENSE.md"):
        shutil.copy2(source / name, prefix / name)
    # Preserve configure's effective closure, not merely our requested switches.
    shutil.copy2(build / "config.h", prefix / "config.h")
    shutil.copy2(build / "config_components.h", prefix / "config_components.h")
    shutil.copy2(build / "ffbuild/config.mak", prefix / "config.mak")
    metadata = {
        "schema_version": 1,
        "target": args.target,
        "source": lock,
        "source_archive": str(archive),
        "prefix": str(prefix),
        "configure": ["--prefix=" + str(prefix), *flags],
        "compiler": (compiler_info.stdout + compiler_info.stderr).strip(),
        "recipe_sha256": digest(Path(__file__).resolve()),
        "license": lock["license"],
        "patches": [],
        "import_libraries": import_libraries,
        "qualification": "candidate capability closure; release requires native corpus and loader verification"
    }
    (prefix / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print("Native install: " + str(prefix))
    print("Build discovery: FFMPEG_DIR=" + str(prefix))
    print("Runtime loader paths are package-relative; FFMPEG_DIR is not a runtime setting.")


if __name__ == "__main__":
    main()
