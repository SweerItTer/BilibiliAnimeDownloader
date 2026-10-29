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

BUILD_ENV = (
    "PATH", "CC", "CFLAGS", "CPPFLAGS", "LDFLAGS", "CXX", "CXXFLAGS", "AR", "AS", "LD", "NM",
    "RANLIB", "STRIP", "PKG_CONFIG", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR",
    "PKG_CONFIG_SYSTEM_INCLUDE_PATH", "PKG_CONFIG_SYSTEM_LIBRARY_PATH", "CONFIG_SITE", "MAKE", "MAKEFLAGS",
    "MFLAGS", "NASM", "CROSS_PREFIX", "SYSROOT", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET", "CPATH",
    "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBRARY_PATH", "LD_LIBRARY_PATH", "INCLUDE", "LIB",
    "VCToolsInstallDir", "VCToolsVersion", "WindowsSDKVersion", "WindowsSdkDir",
    "WindowsSDKLibVersion", "UniversalCRTSdkDir", "UCRTVersion", "VCINSTALLDIR", "VisualStudioVersion",
    "VSCMD_VER", "VSCMD_ARG_TGT_ARCH", "MSYSTEM", "MSYS2_PATH_TYPE",
)


def hash_text(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def build_fingerprint(lock, archive_sha256, target, extra_version, prefix, configure,
                      recipe_sha256, compiler_identity, environment):
    controlled_environment = {
        name: hash_text(environment[name]) if name in environment else None
        for name in BUILD_ENV
    }
    inputs = {
        "source": lock,
        "source_archive_sha256": archive_sha256,
        "target": target,
        "extra_version": extra_version,
        "prefix_sha256": hash_text(str(prefix)),
        "configure": configure,
        "recipe_sha256": recipe_sha256,
        "compiler": compiler_identity,
        "environment_sha256": controlled_environment,
    }
    encoded = json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def installed_files(prefix):
    inventory = []
    root = prefix.resolve()
    for path in sorted(prefix.rglob("*")):
        if path == prefix / "build-info.json":
            continue
        if path.is_symlink():
            if path.is_dir():
                raise ValueError("native install contains a directory symlink")
            target = path.resolve(strict=True)
            if not target.is_relative_to(root):
                raise ValueError("native install symlink escapes its prefix")
            record = {"path": path.relative_to(prefix).as_posix(), "sha256": digest(path),
                      "symlink_target": os.readlink(path)}
        elif path.is_file():
            record = {"path": path.relative_to(prefix).as_posix(), "sha256": digest(path)}
        elif not path.is_dir():
            raise ValueError("native install contains a non-file output")
        else:
            continue
        inventory.append(record)
    return inventory


def native_cache_valid(prefix, expected):
    try:
        info_path = prefix / "build-info.json"
        if info_path.is_symlink() or not info_path.is_file():
            return False
        info = json.loads(info_path.read_text(encoding="utf-8"))
        if any(info.get(key) != value for key, value in expected.items()):
            return False
        records = info.get("installed_files")
        if not isinstance(records, list) or not records or records != installed_files(prefix):
            return False
        files = {record["path"]: record["sha256"] for record in records}
        common = ("config.h", "config_components.h", "config.mak", "COPYING.LGPLv2.1", "LICENSE.md")
        if not all(name in files for name in common):
            return False
        libraries = ("avformat", "avcodec", "avutil")
        if expected["target"].endswith("windows-msvc"):
            if not all(any(path.startswith("bin/") and Path(path).name.startswith(name + "-")
                           and Path(path).suffix.lower() == ".dll" for path in files) for name in libraries):
                return False
            imports = info.get("import_libraries")
            if not isinstance(imports, list) or {item.get("installed") for item in imports} != {
                    f"lib/{name}.lib" for name in libraries}:
                return False
            for item in imports:
                if files.get(item.get("source")) != item.get("source_sha256") or \
                        files.get(item.get("installed")) != item.get("installed_sha256"):
                    return False
        elif not all(any(path.startswith("lib/") and Path(path).name.startswith("lib" + name + ".so")
                         for path in files) for name in libraries):
            return False
        return True
    except (OSError, ValueError, TypeError, KeyError, AttributeError, json.JSONDecodeError):
        return False

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
    if archive.exists() and digest(archive) != lock["sha256"]:
        archive.unlink()
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
    flags = list(lock["configure"])
    if args.extra_version:
        flags.append("--extra-version=" + args.extra_version)
    # No decoder/encoder API is exposed to the app. Narrow probing decoders are
    # retained because libavformat may need them to identify legacy FLV streams.
    if windows:
        flags += ["--toolchain=msvc", "--arch=x86_64", "--target-os=win64", "--extra-cflags=-MD"]
    compiler = "cl" if windows else "cc"
    compiler_info = subprocess.run([compiler] if windows else [compiler, "--version"], capture_output=True, text=True)
    compiler_path = shutil.which(compiler)
    configured_tools = []
    for command_part in shlex.split(os.environ.get("CC", compiler), posix=os.name != "nt"):
        configured_path = shutil.which(command_part)
        if configured_path:
            configured_path = Path(configured_path).resolve()
            configured_tools.append({
                "path_sha256": hash_text(str(configured_path)),
                "binary_sha256": digest(configured_path) if configured_path.is_file() else None,
            })
    build_tools = {}
    build_tool_names = ("make", "nasm", "pkg-config", "pkgconf")
    if windows:
        build_tool_names += ("bash", "cygpath", "link", "lib")
    for name in build_tool_names:
        tool_path = shutil.which(name)
        if tool_path:
            tool_path = Path(tool_path).resolve()
            build_tools[name] = {
                "path_sha256": hash_text(str(tool_path)),
                "binary_sha256": digest(tool_path) if tool_path.is_file() else None,
            }
    compiler_identity = {
        "path_sha256": hash_text(str(Path(compiler_path).resolve())) if compiler_path else None,
        "binary_sha256": digest(Path(compiler_path).resolve()) if compiler_path and Path(compiler_path).is_file() else None,
        "version_sha256": hash_text(compiler_info.stdout + compiler_info.stderr),
        "configured_tools": configured_tools,
        "build_tools": build_tools,
    }
    recipe_sha256 = digest(Path(__file__).resolve())
    configure = ["--prefix=" + str(prefix), *flags]
    fingerprint = build_fingerprint(
        lock, digest(archive), args.target, args.extra_version, prefix, configure,
        recipe_sha256, compiler_identity, os.environ,
    )
    expected = {
        "schema_version": 1,
        "target": args.target,
        "source": lock,
        "source_archive": str(archive),
        "prefix": str(prefix),
        "configure": configure,
        "compiler": (compiler_info.stdout + compiler_info.stderr).strip(),
        "recipe_sha256": recipe_sha256,
        "license": lock["license"],
        "patches": [],
        "qualification": "candidate capability closure; release requires native corpus and loader verification",
        "build_fingerprint": fingerprint,
    }
    if native_cache_valid(prefix, expected):
        print("Native install reused: " + str(prefix))
        print("Build discovery: FFMPEG_DIR=" + str(prefix))
        print("Runtime loader paths are package-relative; FFMPEG_DIR is not a runtime setting.")
        return

    work = prefix.parent / (prefix.name + "-build")
    if work.is_symlink():
        raise SystemExit(f"native build workspace must not be a symlink: {work}")
    source = work / ("ffmpeg-" + lock["version"])
    build = work / "objects"
    if any(path == archive or path in archive.parents or archive in path.parents
           for path in (source, build)):
        raise SystemExit("--source-cache must not overlap native build inputs")
    if any(path == prefix or path in prefix.parents or prefix in path.parents
           for path in (source, build)):
        raise SystemExit("--prefix must not overlap native build inputs")
    for path in (source, build):
        if path.is_symlink():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)
        elif path.exists():
            path.unlink()
    work.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as bundle:
        bundle.extractall(work, filter="data")
    build.mkdir(exist_ok=True)
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
        "configure": configure,
        "compiler": expected["compiler"],
        "recipe_sha256": recipe_sha256,
        "license": lock["license"],
        "patches": [],
        "import_libraries": import_libraries,
        "qualification": "candidate capability closure; release requires native corpus and loader verification",
        "build_fingerprint": fingerprint,
        "installed_files": installed_files(prefix),
    }
    (prefix / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print("Native install: " + str(prefix))
    print("Build discovery: FFMPEG_DIR=" + str(prefix))
    print("Runtime loader paths are package-relative; FFMPEG_DIR is not a runtime setting.")


if __name__ == "__main__":
    main()
