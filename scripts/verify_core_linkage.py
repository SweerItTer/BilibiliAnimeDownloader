#!/usr/bin/env python3
"""Build and prove an independent bili_core Rust SDK consumer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tomllib
import re
import shutil
import subprocess
import sys
import tempfile
import shlex
import tarfile

from collect_licenses import REGISTRY_PREFIX, lock_checksums, registry_archive, registry_root
from verify_package import WINDOWS_SYSTEM, parse_windows_dependencies

ROOT = Path(__file__).resolve().parents[1]
MODES = ("rlib", "dylib")
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")
SOURCE_BUILD_ROOT = Path("..") / "bili-sdk-build"
SYSTEM_LIBS = {"libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", "libutil.so.1", "libresolv.so.2", "ld-linux-x86-64.so.2"}


def source_build_target_dir(target, mode):
    return ROOT / "target" / "sdk-build" / target / "sourceconsumer" / mode


def run(args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, text=True, **kwargs)


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

def source_digest():
    digest = hashlib.sha256()
    for path in sorted((ROOT / "bili_core").glob("Cargo.toml")) + sorted((ROOT / "bili_core/src").rglob("*")):
        if path.is_file():
            digest.update(path.relative_to(ROOT / "bili_core").as_posix().encode())
            digest.update(bytes.fromhex(sha256(path)))
    return digest.hexdigest()

def ffmpeg_headers(prefix):
    versions = {}
    for component in ("avformat", "avcodec", "avutil"):
        for filename in (f"lib{component}/version_major.h", f"lib{component}/version.h"):
            path = prefix / "include" / filename
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            macros = re.findall(r"^\s*#\s*define\s+(LIBAV[A-Z]+_VERSION(?:_MAJOR)?)\s+(\d+)\b", text, re.M)
            if macros:
                versions[component] = {key: int(value) for key, value in macros}
                break
    if len(versions) != 3:
        raise RuntimeError("native FFmpeg prefix is missing avformat/avcodec/avutil version headers")
    return versions



def copy_core_source(dst):
    source = ROOT / "bili_core"
    (dst / "src").mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / "Cargo.toml", dst / "Cargo.toml")
    for path in (source / "src").rglob("*"):
        if path.is_file():
            target = dst / "src" / path.relative_to(source / "src")
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


def rust_vv():
    return subprocess.check_output(["rustc", "-Vv"], text=True).strip()

def native_info(prefix):
    path = prefix / "build-info.json"
    if not path.is_file():
        raise RuntimeError(f"native prefix has no build-info.json: {prefix}")
    info = json.loads(path.read_text(encoding="utf-8"))
    pin = json.loads((ROOT / "scripts/native-lock.json").read_text(encoding="utf-8"))
    if info.get("source") != pin:
        raise RuntimeError("native prefix does not match the pinned FFmpeg source and SHA-256")
    if info.get("recipe_sha256") != sha256(ROOT / "scripts/build_ffmpeg.py"):
        raise RuntimeError("native prefix was built by a different FFmpeg recipe")
    return info


def set_native_env(prefix):
    env = os.environ.copy()
    env["FFMPEG_DIR"] = str(prefix)
    pkgconfig = prefix / "lib" / "pkgconfig"
    env["PKG_CONFIG_PATH"] = str(pkgconfig) + os.pathsep + env.get("PKG_CONFIG_PATH", "")
    env["PKG_CONFIG_LIBDIR"] = str(pkgconfig)
    return env


def metadata(target):
    return json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--filter-platform", target, "--format-version", "1"],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
    ))


def build_with_artifacts(command, env):
    process = subprocess.run(
        [str(arg) for arg in command + ["--message-format=json-render-diagnostics"]],
        cwd=ROOT,
        env=env,
        check=True,
        text=True,
        encoding="utf-8",
        stdout=subprocess.PIPE,
    )
    artifacts, build_scripts = [], []
    for line in process.stdout.splitlines():
        if line.startswith("{"):
            event = json.loads(line)
            if event.get("reason") == "compiler-artifact":
                artifacts.append(event)
            elif event.get("reason") == "build-script-executed":
                build_scripts.append(event)
    return artifacts, build_scripts


def cargo_package_graph(cargo_metadata, package_name):
    package = next(p for p in cargo_metadata["packages"] if p["name"] == package_name)
    nodes = {node["id"]: node for node in cargo_metadata["resolve"]["nodes"]}
    reached = set()
    pending = [package["id"]]
    while pending:
        package_id = pending.pop()
        if package_id in reached:
            continue
        reached.add(package_id)
        pending.extend(
            dependency["pkg"]
            for dependency in nodes[package_id]["deps"]
            if any(kind.get("kind") is None for kind in dependency.get("dep_kinds", []))
        )
    return package["id"], reached, nodes


def compiler_artifact_paths(artifacts, reachable):
    for artifact in artifacts:
        if artifact["package_id"] not in reachable:
            continue
        is_macro = "proc-macro" in artifact["target"]["kind"]
        for filename in artifact["filenames"]:
            path = Path(filename)
            if path.is_file() and (
                path.suffix in (".rlib", ".rmeta")
                or (is_macro and path.suffix in (".so", ".dll", ".dylib"))
            ):
                yield path


def copy_registry_import_libraries(build_scripts, packages, reachable, output, target):
    if not target.endswith("windows-msvc"):
        return []
    checksums = lock_checksums()
    records = []
    for event in build_scripts:
        package = packages.get(event["package_id"])
        if event["package_id"] not in reachable or not package:
            continue
        source = package.get("source") or ""
        if not source.startswith(REGISTRY_PREFIX):
            continue
        registry, crate = registry_root(package["manifest_path"])
        search_paths = []
        for linked_path in event.get("linked_paths", []):
            kind, separator, value = linked_path.partition("=")
            if not separator or kind != "native":
                continue
            declared = Path(value).absolute()
            path = declared.resolve(strict=True)
            if declared.is_relative_to(crate) and not path.is_relative_to(crate):
                raise RuntimeError(f"registry import search path escapes its crate: {declared}")
            # Generated archives and the separately copied FFmpeg prefix are not
            # registry-provided import inputs.
            if path.is_relative_to(crate):
                search_paths.append(path)
        if not search_paths:
            continue
        checksum = checksums.get((package["name"], package["version"], source))
        if not checksum:
            raise RuntimeError(f"missing locked registry checksum for {package['id']}")
        archive = registry_archive(registry, package["name"], package["version"], checksum)
        with tarfile.open(archive, "r:gz") as tar:
            for directory in sorted(set(search_paths)):
                for candidate in sorted(directory.glob("*.lib")):
                    path = candidate.resolve(strict=True)
                    if not path.is_relative_to(crate) or not path.is_file():
                        raise RuntimeError(f"registry import library escapes its crate: {candidate}")
                    member_name = f"{package['name']}-{package['version']}/{path.relative_to(crate).as_posix()}"
                    try:
                        member = tar.getmember(member_name)
                    except KeyError as error:
                        raise RuntimeError(f"registry import library is absent from locked archive: {member_name}") from error
                    if not member.isfile():
                        raise RuntimeError(f"registry import library is not an archive file: {member_name}")
                    stream = tar.extractfile(member)
                    if stream is None:
                        raise RuntimeError(f"cannot read registry import library: {member_name}")
                    data = stream.read()
                    if data != path.read_bytes():
                        raise RuntimeError(f"registry import library differs from locked archive: {member_name}")
                    destination = output / "native/lib" / path.name
                    if destination.exists():
                        if destination.read_bytes() != data:
                            raise RuntimeError(f"conflicting SDK import library: {path.name}")
                    else:
                        destination.write_bytes(data)
                    records.append({
                        "package": package["name"], "version": package["version"],
                        "source": source, "archive_sha256": checksum,
                        "archive_member": member_name,
                        "path": destination.relative_to(output).as_posix(),
                        "sha256": hashlib.sha256(data).hexdigest(),
                        "input_kind": "Cargo-provided precompiled import library; not CI-rebuilt, FFmpeg source or Windows redist",
                    })
    return records


def target_artifact(artifacts, package_id, name, suffix):
    for artifact in artifacts:
        if artifact["package_id"] != package_id or artifact["target"]["name"] != name:
            continue
        match = next((Path(path) for path in artifact["filenames"] if Path(path).suffix == suffix), None)
        if match:
            return match
    return None


def copy_runtime_closure(executable, libdir, env, windows):
    libdir.mkdir(parents=True, exist_ok=True)
    if windows:
        tool = shutil.which("dumpbin")
        if not tool:
            raise RuntimeError("dumpbin is required to inspect PE dependency closure")
        redist_value = os.environ.get("VCToolsRedistDir")
        search = [Path(env["FFMPEG_DIR"]) / "bin", libdir, libdir.parent / "lib", Path(env.get("RUST_TARGET_LIBDIR", ""))]
        if redist_value:
            redist = Path(redist_value) / "x64" / "Microsoft.VC143.CRT"
            if redist.is_dir():
                search.append(redist)
        pending = [Path(executable)]
        copied = set()
        closure = {}
        while pending:
            binary = pending.pop()
            text = subprocess.check_output([tool, "/dependents", str(binary)], text=True)
            names = parse_windows_dependencies(text)
            for name in names:
                key = name.lower()
                if key in copied or key in WINDOWS_SYSTEM or key.startswith(("api-ms-win-", "ext-ms-win-")):
                    continue
                candidate = next((directory / name for directory in search if directory.is_dir() and (directory / name).is_file()), None)
                if candidate is None:
                    raise RuntimeError(f"unresolved non-system PE dependency (no System32 fallback): {name}")
                destination = libdir / name
                if not destination.exists():
                    shutil.copy2(candidate, destination)
                closure[name] = candidate.name
                copied.add(key)
                pending.append(destination)
        return closure
    ldd = subprocess.check_output(["ldd", str(executable)], text=True, env=env)
    if "not found" in ldd:
        raise RuntimeError("consumer has unresolved shared dependencies")
    for line in ldd.splitlines():
        match = re.search(r"=>\s+(/[^ ]+)", line)
        if match:
            source = Path(match.group(1))
            if source.name not in SYSTEM_LIBS and not (libdir / source.name).exists():
                shutil.copy2(source, libdir / source.name)
    needed = sorted({Path(match.group(1)).name for line in ldd.splitlines() if (match := re.search(r"=>\s+(/[^ ]+)", line))})
    return {"resolved": needed}


def copy_native_sdk(prefix, output, target):
    native = output / "native"
    include = prefix / "include"
    library = prefix / "lib"
    if not include.is_dir() or not library.is_dir():
        raise RuntimeError("native FFmpeg prefix is missing development headers or libraries")
    shutil.copytree(include, native / "include")
    native_lib = native / "lib"
    native_lib.mkdir(parents=True, exist_ok=True)
    is_windows = target.endswith("windows-msvc")
    runtime_dir = output / ("bin" if is_windows else "lib")
    runtime_dir.mkdir(parents=True, exist_ok=True)
    suffixes = (".lib",) if is_windows else (".so",)
    for path in library.iterdir():
        if not path.is_file() or not (path.suffix in suffixes or (not is_windows and ".so." in path.name)):
            continue
        if is_windows:
            shutil.copy2(path, native_lib / path.name)
            continue
        runtime_file = runtime_dir / path.name
        if path.is_symlink():
            link = Path(os.readlink(path))
            if link.is_absolute() or not path.resolve(strict=True).is_relative_to(library.resolve()):
                raise RuntimeError(f"native library symlink escapes its prefix: {path.name}")
            runtime_file.symlink_to(link)
        else:
            shutil.copy2(path, runtime_file)
        (native_lib / path.name).symlink_to(Path("../../lib") / path.name)
    pkgconfig = library / "pkgconfig"
    if not pkgconfig.is_dir():
        raise RuntimeError("native FFmpeg prefix has no pkg-config development metadata")
    sdk_pkgconfig = native_lib / "pkgconfig"
    sdk_pkgconfig.mkdir()
    for source_pc in pkgconfig.glob("*.pc"):
        text = source_pc.read_text(encoding="utf-8")
        rewritten = text
        for field, value in (
            ("prefix", "${pcfiledir}/../.."),
            ("libdir", "${prefix}/lib"),
            ("includedir", "${prefix}/include"),
        ):
            rewritten, count = re.subn(rf"(?m)^{field}=.*$", f"{field}={value}", rewritten, count=1)
            if count != 1:
                raise RuntimeError(f"native pkg-config file has no relocatable {field}: {source_pc.name}")
        (sdk_pkgconfig / source_pc.name).write_text(rewritten, encoding="utf-8")
    if is_windows:
        source_dll_dir = prefix / "bin"
        if source_dll_dir.is_dir():
            for dll in source_dll_dir.glob("*.dll"):
                if dll.name.lower().startswith(("avformat", "avcodec", "avutil", "libavformat", "libavcodec", "libavutil")):
                    shutil.copy2(dll, runtime_dir / dll.name)
    return native

def prepare_sdk_consumer(output, target):
    source = output / "source"
    source.mkdir(parents=True, exist_ok=True)
    copy_core_source(source / "bili_core")
    (source / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
    consumer = output / "consumer"
    consumer.mkdir(parents=True, exist_ok=True)
    manifest = consumer / "Cargo.toml"
    shutil.copy2(ROOT / "scripts/sdk_consumer/Cargo.toml", manifest)
    (consumer / "src").mkdir(exist_ok=True)
    src = consumer / "src" / "main.rs"
    shutil.copy2(ROOT / "scripts/sdk_consumer/src/main.rs", src)
    shutil.copy2(ROOT / "scripts/sdk_consumer/README.md", consumer / "README.md")
    lock = consumer / "Cargo.lock"
    lock.write_bytes((ROOT / "Cargo.lock").read_bytes())
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", target, "--manifest-path", manifest],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
    ))
    verify_consumer_lock(lock)
    return consumer, src, lock, metadata


def write_source_build_recipe(consumer, target, env, mode):
    relative_build_dir = SOURCE_BUILD_ROOT / target / mode
    values = {
        "FFMPEG_DIR": "native",
        "PKG_CONFIG_PATH": "native/lib/pkgconfig",
        "PKG_CONFIG_LIBDIR": "native/lib/pkgconfig",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": os.environ.get("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3"),
        "CARGO_PROFILE_RELEASE_LTO": os.environ.get("CARGO_PROFILE_RELEASE_LTO", "false"),
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": os.environ.get("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16"),
        "CARGO_PROFILE_RELEASE_DEBUG": os.environ.get("CARGO_PROFILE_RELEASE_DEBUG", "false"),
        "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS": os.environ.get("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "false"),
        "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS": os.environ.get("CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "false"),
        "CARGO_PROFILE_RELEASE_STRIP": os.environ.get("CARGO_PROFILE_RELEASE_STRIP", "none"),
        "CARGO_PROFILE_RELEASE_PANIC": "unwind",
        "RUSTFLAGS": env["RUSTFLAGS"],
        "CARGO_TARGET_DIR": str(relative_build_dir),
    }
    native_paths = {"FFMPEG_DIR", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR"}
    # ffmpeg-sys-next does not watch FFMPEG_DIR; cached native link-search output
    # can otherwise retain a removed staging path.
    if target.endswith("windows-msvc"):
        values.pop("CARGO_TARGET_DIR")
        clean = f'cargo clean --locked -p ffmpeg-sys-next --release --target {target} --manifest-path consumer/Cargo.toml --target-dir "%~fI"'
        build = f'cargo build --locked --release --target {target} --manifest-path consumer/Cargo.toml --target-dir "%~fI"'
        prefix = " && ".join(
            f'set "{key}={"%CD%/" if key in native_paths else ""}{value}"'
            for key, value in values.items()
        )

        def build_step(target_dir):
            return f'for %I in ("{target_dir}") do (set "CARGO_TARGET_DIR=%~fI" && {prefix} && {clean} && {build})'

        recipe = (
            f'if defined CARGO_TARGET_DIR ({build_step("%CARGO_TARGET_DIR%")}) '
            f'else ({build_step(relative_build_dir)})\n'
        )
    else:
        prefix = " ".join(
            f'{key}="${{CARGO_TARGET_DIR:-{value}}}"' if key == "CARGO_TARGET_DIR"
            else f'{key}="$PWD/{value}"' if key in native_paths
            else f"{key}={shlex.quote(value)}"
            for key, value in values.items()
        )
        target_dir_arg = f'"${{CARGO_TARGET_DIR:-{relative_build_dir}}}"'
        clean = f'cargo clean --locked -p ffmpeg-sys-next --release --target {target} --manifest-path consumer/Cargo.toml --target-dir {target_dir_arg}'
        build = f'cargo build --locked --release --target {target} --manifest-path consumer/Cargo.toml --target-dir {target_dir_arg}'
        recipe = f"{prefix} {clean} && {prefix} {build}\n"
    (consumer / "source-build-command.txt").write_text(
        recipe,
        encoding="utf-8",
    )


def build_source_consumer(output, prefix, target, profile):
    native = copy_native_sdk(prefix, output, target)
    consumer_dir, src, consumer_lock, source_metadata = prepare_sdk_consumer(output, target)
    env = set_native_env(prefix)
    env["RUST_TARGET_LIBDIR"] = str(rust_target_libdir(target))
    env["RUSTFLAGS"] = rustflags(env, output)
    build_dir = ROOT / "target" / "sdk-build" / target / "rlib"
    command = ["cargo", "build", "-p", "bili_core", "--lib", "--locked", "--target", target, "--target-dir", build_dir]
    if profile == "release":
        command.append("--release")
    artifacts, build_scripts = build_with_artifacts(command, env)
    cargo_metadata = metadata(target)
    core_id, reachable, nodes = cargo_package_graph(cargo_metadata, "bili_core")
    core_rlib = target_artifact(artifacts, core_id, "bili_core", ".rlib")
    if core_rlib is None:
        raise RuntimeError("Cargo compiler-artifact output did not report bili_core's selected rlib")
    workspace_packages = {package["id"]: package for package in cargo_metadata["packages"]}
    source_core_id, source_reachable, _ = cargo_package_graph(source_metadata, "bili_core")
    source_packages = {package["id"]: package for package in source_metadata["packages"]}
    runtime_ids = {
        (source_packages[item]["name"], source_packages[item]["version"], source_packages[item].get("source"))
        for item in source_reachable if item != source_core_id and source_packages[item].get("source")
    }
    reachable = {
        package_id for package_id in reachable
        if package_id == core_id or (
            workspace_packages[package_id]["name"],
            workspace_packages[package_id]["version"],
            workspace_packages[package_id].get("source"),
        ) in runtime_ids
    }

    libdir, bindir = output / "lib", output / "bin"
    libdir.mkdir(parents=True, exist_ok=True)
    bindir.mkdir(parents=True, exist_ok=True)
    for path in compiler_artifact_paths(artifacts, reachable):
        shutil.copy2(path, libdir / path.name)
    import_inputs = copy_registry_import_libraries(build_scripts, workspace_packages, reachable, output, target)
    shutil.copy2(core_rlib, libdir / core_rlib.name)
    direct = {dep["name"]: dep["pkg"] for dep in nodes[core_id]["deps"]}
    direct_artifacts = {}
    for name in ("tokio", "serde_json"):
        package_id = direct.get(name)
        package = workspace_packages.get(package_id)
        artifact = target_artifact(artifacts, package_id, package["name"], ".rlib") if package else None
        if artifact is None:
            raise RuntimeError(f"Cargo compiler-artifact output omitted {name}'s selected rlib")
        direct_artifacts[name] = artifact

    consumer_env = set_native_env(native)
    consumer_env["RUST_TARGET_LIBDIR"] = str(rust_target_libdir(target))
    consumer_env["RUSTFLAGS"] = rustflags(consumer_env, output)
    executable = bindir / ("bili-core-sdk-consumer.exe" if target.endswith("windows-msvc") else "bili-core-sdk-consumer")
    proof_command = [
        "rustc", "--edition=2021", "consumer/src/main.rs", "--target", target,
        *release_rustc_options(), *shlex.split(consumer_env["RUSTFLAGS"]),
        "-L", "dependency=lib", "-L", "native=native/lib",
        f"--extern=bili_core=lib/{core_rlib.name}",
        f"--extern=tokio=lib/{direct_artifacts['tokio'].name}",
        f"--extern=serde_json=lib/{direct_artifacts['serde_json'].name}",
        "-o", f"bin/{executable.name}",
    ]
    run(proof_command, cwd=output, env=consumer_env)
    (consumer_dir / "build-command.txt").write_text(shlex.join(proof_command) + "\n", encoding="utf-8")
    write_source_build_recipe(consumer_dir, target, consumer_env, "rlib")
    return executable, consumer_env, consumer_lock, artifacts, reachable, import_inputs


def rustflags(env, output):
    flags = shlex.split(env.get("RUSTFLAGS", ""))
    flags += ["-C", "panic=unwind", "-C", "prefer-dynamic"]
    if sys.platform != "win32":
        flags += ["-C", "link-arg=-Wl,-rpath,$ORIGIN/../lib"]
    return " ".join(flags)


def rust_target_libdir(target):
    return Path(subprocess.check_output(["rustc", "--print", "target-libdir", "--target", target], text=True).strip())


def release_rustc_options():
    if os.environ.get("CARGO_PROFILE_RELEASE_PANIC", "unwind") != "unwind":
        raise RuntimeError("SDK consumers require the core's panic=unwind build profile")
    lto = os.environ.get("CARGO_PROFILE_RELEASE_LTO", "false").lower()
    lto = {"false": "off", "true": "fat"}.get(lto, lto)
    debug = os.environ.get("CARGO_PROFILE_RELEASE_DEBUG", "false").lower()
    debug = {"false": "0", "true": "2"}.get(debug, debug)
    strip = os.environ.get("CARGO_PROFILE_RELEASE_STRIP", "none").lower()
    strip = {"false": "none", "true": "symbols"}.get(strip, strip)
    return [
        "-C", f"opt-level={os.environ.get('CARGO_PROFILE_RELEASE_OPT_LEVEL', '3')}",
        "-C", f"codegen-units={os.environ.get('CARGO_PROFILE_RELEASE_CODEGEN_UNITS', '16')}",
        "-C", f"lto={lto}",
        "-C", f"debug-assertions={os.environ.get('CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS', 'false')}",
        "-C", f"overflow-checks={os.environ.get('CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS', 'false')}",
        "-C", f"debuginfo={debug}",
        "-C", f"strip={strip}",
        "-C", "panic=unwind",
    ]

def build_dynamic_consumer(output, prefix, target, profile):
    env = set_native_env(prefix)
    env["RUST_TARGET_LIBDIR"] = str(rust_target_libdir(target))
    env["RUSTFLAGS"] = rustflags(env, output)
    cargo = ["cargo", "build", "-p", "bili_core", "--lib", "--locked", "--target", target, "--target-dir", ROOT / "target"]
    if profile == "release":
        cargo.append("--release")
    artifacts, build_scripts = build_with_artifacts(cargo, env)
    cargo_metadata = metadata(target)
    core_id, reachable, nodes = cargo_package_graph(cargo_metadata, "bili_core")
    dylib_suffix = ".dll" if target.endswith("windows-msvc") else ".so"
    core = target_artifact(artifacts, core_id, "bili_core", dylib_suffix)
    if core is None:
        raise RuntimeError("Cargo compiler-artifact output did not report bili_core's selected dylib")
    core_node = nodes[core_id]
    direct = {dep["name"]: dep["pkg"] for dep in core_node["deps"]}
    tokio_id = direct.get("tokio")
    serde_json_id = direct.get("serde_json")
    tokio_pkg = next((p for p in cargo_metadata["packages"] if p["id"] == tokio_id), None)
    serde_json_pkg = next((p for p in cargo_metadata["packages"] if p["id"] == serde_json_id), None)
    if not tokio_pkg or not serde_json_pkg:
        raise RuntimeError("Cargo metadata omitted direct Tokio/serde_json dependency identities")

    libdir, bindir = output / "lib", output / "bin"
    libdir.mkdir(parents=True, exist_ok=True)
    bindir.mkdir(parents=True, exist_ok=True)
    for path in compiler_artifact_paths(artifacts, reachable - {core_id}):
        shutil.copy2(path, libdir / path.name)
    packages = {package["id"]: package for package in cargo_metadata["packages"]}
    import_inputs = copy_registry_import_libraries(build_scripts, packages, reachable, output, target)
    tokio = target_artifact(artifacts, tokio_id, tokio_pkg["name"], ".rlib")
    serde_json = target_artifact(artifacts, serde_json_id, serde_json_pkg["name"], ".rlib")
    if tokio is None or serde_json is None:
        raise RuntimeError("compiler-artifact output omitted the exact Tokio/serde_json rlibs")
    tokio = libdir / tokio.name
    serde_json = libdir / serde_json.name
    core_dir = bindir if target.endswith("windows-msvc") else libdir
    core_copy = core_dir / core.name
    shutil.copy2(core, core_copy)
    if target.endswith("windows-msvc"):
        for artifact in artifacts:
            if artifact["package_id"] == core_id:
                for filename in artifact["filenames"]:
                    path = Path(filename)
                    if path.suffix == ".lib":
                        shutil.copy2(path, bindir / path.name)
    consumer, src, consumer_lock, _ = prepare_sdk_consumer(output, target)
    native = output / "native"
    consumer_env = set_native_env(native)
    consumer_env["RUST_TARGET_LIBDIR"] = str(rust_target_libdir(target))
    consumer_env["RUSTFLAGS"] = rustflags(consumer_env, output)
    executable = bindir / ("bili-core-sdk-consumer.exe" if target.endswith("windows-msvc") else "bili-core-sdk-consumer")
    core_sdk_path = f"bin/{core.name}" if target.endswith("windows-msvc") else f"lib/{core.name}"
    command = [
        "rustc", "--edition=2021", "consumer/src/main.rs", "--target", target,
        *release_rustc_options(), *shlex.split(consumer_env["RUSTFLAGS"]),
        "-L", "dependency=lib", "-L", "native=native/lib",
        f"--extern=bili_core={core_sdk_path}",
        f"--extern=tokio=lib/{tokio.name}", f"--extern=serde_json=lib/{serde_json.name}",
        "-o", f"bin/{executable.name}",
    ]
    if target.endswith("windows-msvc"):
        command += ["-L", "dependency=bin"]
    run(command, cwd=output, env=consumer_env)
    (consumer / "build-command.txt").write_text(shlex.join(command) + "\n", encoding="utf-8")
    write_source_build_recipe(consumer, target, consumer_env, "dylib")
    return executable, core_copy, artifacts, reachable, consumer_env, consumer_lock, import_inputs


def inspect_dynamic(executable, core, output, target, env):
    report = {}
    if target.endswith("windows-msvc"):
        tool = shutil.which("dumpbin")
        if not tool:
            raise RuntimeError("dumpbin is required to inspect Windows PE dependencies")
        report["pe"] = subprocess.check_output([tool, "/dependents", str(executable)], text=True)
        report["core_pe"] = subprocess.check_output([tool, "/dependents", str(core)], text=True)
        if core.name.lower() not in report["pe"].lower():
            raise RuntimeError("consumer PE does not depend on the selected core DLL")
    else:
        readelf = shutil.which("readelf")
        if not readelf:
            raise RuntimeError("readelf is required to inspect ELF dependencies")
        report["elf_before_patch"] = subprocess.check_output([readelf, "-d", str(executable)], text=True)
        report["core_elf_before_patch"] = subprocess.check_output([readelf, "-d", str(core)], text=True)
        needed = re.findall(r"Shared library: \[(.*?)\]", report["elf_before_patch"])
        if core.name not in needed:
            raise RuntimeError("consumer ELF DT_NEEDED does not name the selected core dylib")
    report["dependency_closure"] = copy_runtime_closure(executable, runtime_library_dir(output, target), env, target.endswith("windows-msvc"))
    return report
def inspect_rlib(executable, output, target, env):
    if target.endswith("windows-msvc"):
        tool = shutil.which("dumpbin")
        if not tool:
            raise RuntimeError("dumpbin is required to inspect the rlib consumer")
        evidence = subprocess.check_output([tool, "/dependents", str(executable)], text=True)
        if "bili_core.dll" in evidence.lower():
            raise RuntimeError("rlib consumer unexpectedly depends on a core DLL")
    else:
        tool = shutil.which("readelf")
        if not tool:
            raise RuntimeError("readelf is required to inspect the rlib consumer")
        evidence = subprocess.check_output([tool, "-d", str(executable)], text=True)
        if re.search(r"Shared library: \[libbili_core[^]]*\.so", evidence):
            raise RuntimeError("rlib consumer unexpectedly depends on a core dylib")
        if "$ORIGIN/../lib" not in evidence:
            raise RuntimeError("rlib consumer lacks the SDK-relative ELF RUNPATH")
    closure = copy_runtime_closure(executable, runtime_library_dir(output, target), env, target.endswith("windows-msvc"))
    return {"consumer_dependencies": evidence, "dependency_closure": closure}


def core_version():
    with (ROOT / "bili_core/Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["package"]["version"]


def dependency_versions(target):
    cargo_metadata = metadata(target)
    root_id, reached, _ = cargo_package_graph(cargo_metadata, "bili_core")
    packages = {package["id"]: package for package in cargo_metadata["packages"]}
    return sorted(
        (packages[package_id]["name"], packages[package_id]["version"])
        for package_id in reached
        if packages[package_id].get("source")
    )


def core_features(target):
    cargo_metadata = metadata(target)
    package = next(p for p in cargo_metadata["packages"] if p["name"] == "bili_core")
    node = next(n for n in cargo_metadata["resolve"]["nodes"] if n["id"] == package["id"])
    return node["features"]

def verify_consumer_lock(lockfile):
    official = {
        (package["name"], package["version"], package.get("source"), package.get("checksum"))
        for package in tomllib.load((ROOT / "Cargo.lock").open("rb"))["package"]
        if package.get("source")
    }
    consumer = {
        (package["name"], package["version"], package.get("source"), package.get("checksum"))
        for package in tomllib.load(Path(lockfile).open("rb"))["package"]
        if package.get("source")
    }
    if not consumer.issubset(official):
        added = sorted(consumer - official)
        raise RuntimeError(f"independent consumer lock contains packages absent from workspace lock: {added}")

def package_notices(output, prefix):
    notices = output / "notices"
    notices.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", notices / "LICENSE")
    for name in ("COPYING.LGPLv2.1", "LICENSE.md"):
        candidate = prefix / name
        if not candidate.is_file():
            raise RuntimeError(f"native FFmpeg prefix is missing required notice: {name}")
        shutil.copy2(candidate, notices / name)


def runtime_library_dir(output, target):
    return output / ("bin" if target.endswith("windows-msvc") else "lib")

def isolated_runtime_env(env):
    runtime = env.copy()
    for key in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "FFMPEG_DIR", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "RUST_TARGET_LIBDIR"):
        runtime.pop(key, None)
    if sys.platform == "win32":
        runtime["PATH"] = str(Path(os.environ.get("WINDIR", "C:/Windows")) / "System32")
    return runtime

def verify_source_cargo(output, prefix, target, env):
    recipe = (output / "consumer/source-build-command.txt").read_text(encoding="utf-8").strip()
    build_env = env.copy()
    for key in ("FFMPEG_DIR", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "RUST_TARGET_LIBDIR", "RUSTFLAGS"):
        build_env.pop(key, None)
    build_env["CARGO_TARGET_DIR"] = str(source_build_target_dir(target, "rlib"))
    if sys.platform != "win32":
        producer_lib = str(prefix / "lib")
        build_env["LD_LIBRARY_PATH"] = os.pathsep.join(
            path for path in build_env.get("LD_LIBRARY_PATH", "").split(os.pathsep)
            if path and str(Path(path).resolve()) != producer_lib
        )
    if sys.platform == "win32":
        cmd = Path(os.environ.get("WINDIR", "C:/Windows")) / "System32/cmd.exe"
        # CMD consumes its own quoting; list2cmdline escapes would corrupt the SET chain.
        command = f'"{cmd}" /d /c "{recipe}"'
        build = subprocess.run(command, executable=str(cmd), check=True, cwd=output, env=build_env)
    else:
        build = run(["bash", "-c", recipe], cwd=output, env=build_env)
    release = source_build_target_dir(target, "rlib") / target / "release"
    executable = release / ("bili-core-sdk-consumer.exe" if sys.platform == "win32" else "bili-core-sdk-consumer")
    runtime = isolated_runtime_env(build_env)
    for key in ("LIBCLANG_PATH", "BINDGEN_EXTRA_CLANG_ARGS", "PKG_CONFIG", "RUSTFLAGS", "CARGO_TARGET_DIR"):
        runtime.pop(key, None)
    runtime_libraries = [release, runtime_library_dir(output, target)]
    if sys.platform == "win32":
        runtime["PATH"] = os.pathsep.join(map(str, runtime_libraries + [Path(runtime["PATH"])]))
    else:
        runtime["LD_LIBRARY_PATH"] = os.pathsep.join(map(str, runtime_libraries))
    with tempfile.TemporaryDirectory(prefix="bili-sdk-source-state-") as temporary:
        root = Path(temporary)
        state, result = root / "state", root / "output"
        state.mkdir(mode=0o700)
        result.mkdir(mode=0o700)
        runtime.update({
            "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "home/.config"),
            "XDG_DATA_HOME": str(root / "home/.local/share"),
        })
        lifecycle = run([executable, state, result], env=runtime, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if any(state.iterdir()) or any(result.iterdir()):
            raise RuntimeError("source-Cargo readonly consumer modified state or output")
    core_artifacts = [
        path for path in release.iterdir()
        if path.is_file() and path.name in {"libbili_core.rlib", "libbili_core.so", "bili_core.dll", "bili_core.dll.lib"}
    ]
    if not core_artifacts:
        raise RuntimeError("source-Cargo build produced no recipient core artifact")
    return {
        "recipe": "consumer/source-build-command.txt",
        "recipe_command": recipe,
        "sdk_root_at_execution": str(output),
        "source_manifest": "source/bili_core/Cargo.toml",
        "source_manifest_sha256": sha256(output / "source/bili_core/Cargo.toml"),
        "build_exit_code": build.returncode,
        "consumer_executable": str(executable),
        "consumer_sha256": sha256(executable),
        "core_artifacts": [{"path": str(path), "sha256": sha256(path)} for path in core_artifacts],
        "runtime_library_directories": list(map(str, runtime_libraries)),
        "runtime_ffmpeg_dir_present": "FFMPEG_DIR" in runtime,
        "readonly_lifecycle_exit_code": lifecycle.returncode,
        "readonly_state_output_unchanged": True,
        "runtime_stdout": lifecycle.stdout,
        "runtime_stderr": lifecycle.stderr,
        "scope": "recipient-built source consumer with copied SDK native/runtime inputs; not a self-contained app",
    }


def inspect_runtime_search_paths(output):
    runpaths = {}
    if sys.platform == "win32":
        return runpaths
    patchelf = shutil.which("patchelf")
    readelf = shutil.which("readelf")
    if not patchelf or not readelf:
        raise RuntimeError("patchelf and readelf are required to package and verify ELF SDK RUNPATHs")
    for library in (output / "lib").glob("*.so*"):
        if library.is_symlink() or not library.is_file():
            continue
        run([patchelf, "--set-rpath", "$ORIGIN", library])
        tags = subprocess.check_output([readelf, "-d", str(library)], text=True)
        if "$ORIGIN" not in tags:
            raise RuntimeError(f"shared library lacks its own $ORIGIN RUNPATH: {library.name}")
        runpaths[library.name] = tags
    return runpaths


def dependency_lookup_path(env, output, prefix, target):
    lookup = env.copy()
    if sys.platform == "win32":
        lookup["PATH"] = os.pathsep.join((str(output / "lib"), str(output / "bin"), str(prefix / "bin")))
    else:
        lookup["LD_LIBRARY_PATH"] = os.pathsep.join((str(output / "lib"), str(prefix / "lib"), str(rust_target_libdir(target))))
    return lookup


def finalize_elf_inspection(inspection, executable, core, mode, runpaths):
    readelf = shutil.which("readelf")
    if not readelf:
        raise RuntimeError("readelf is required to inspect final ELF SDK artifacts")
    consumer_tags = subprocess.check_output([readelf, "-d", str(executable)], text=True)
    if "$ORIGIN/../lib" not in consumer_tags:
        raise RuntimeError("consumer final RUNPATH is not app-relative")
    inspection["consumer_elf_final"] = consumer_tags
    if mode == "dylib":
        core_tags = subprocess.check_output([readelf, "-d", str(core)], text=True)
        if "$ORIGIN" not in core_tags:
            raise RuntimeError("core final RUNPATH does not resolve adjacent SDK libraries")
        inspection["core_elf_final"] = core_tags
    inspection["shared_library_elf_final"] = runpaths


def runtime_license_inputs(inspection, prefix, target, explicit=()):
    closure = inspection.get("dependency_closure", {})
    names = set(closure.get("resolved", []))
    names.update(closure.keys())
    inputs = [prefix / "COPYING.LGPLv2.1", prefix / "LICENSE.md"]
    supplied = [Path(path).resolve(strict=True) for path in explicit]
    if any(not path.is_file() for path in supplied):
        raise RuntimeError("caller-supplied runtime license input is not a file")
    if target.endswith("windows-msvc"):
        return sorted({path.resolve() for path in inputs + supplied if path.is_file()})

    runtime = {name.lower() for name in names}
    docs = Path("/usr/share/doc")
    common = Path("/usr/share/common-licenses")
    exceptions = list(docs.glob("gcc-*/COPYING.RUNTIME"))
    distro_copyrights = list((docs / "libgcc-s1").glob("copyright")) + list(docs.glob("libstdc++*/copyright"))
    for path in distro_copyrights:
        text = path.read_text(encoding="utf-8")
        if (
            re.search(r"GCC RUNTIME LIBRARY EXCEPTION\s+Version 3\.1, 31 March 2009", text)
            and "presumption that third-party software is unaffected by the copyleft\nrequirements of the license of GCC." in text
        ):
            exceptions.append(path)
    gpl_texts = list(docs.glob("gcc-*/COPYING3")) + [common / "GPL-3"]
    if any(name.startswith("libgcc_s.so") for name in runtime):
        distro_texts = list((docs / "libgcc-s1").glob("copyright"))
        if not exceptions or not any(path.is_file() for path in gpl_texts):
            raise RuntimeError("copied libgcc_s has no complete installed GPL/runtime-exception license texts")
        inputs.extend(exceptions + gpl_texts + distro_texts)
    if any(name.startswith("libstdc++.so") for name in runtime):
        distro_texts = list(docs.glob("libstdc++*/copyright"))
        lgpl_texts = list(docs.glob("gcc-*/COPYING.LIB")) + [common / "LGPL-3"]
        if not exceptions or not any(path.is_file() for path in gpl_texts) or not any(path.is_file() for path in lgpl_texts):
            raise RuntimeError("copied libstdc++ has no complete installed GPL/LGPL/runtime-exception license texts")
        inputs.extend(exceptions + gpl_texts + lgpl_texts + distro_texts)
    return sorted({path.resolve() for path in inputs + supplied if path.is_file()})


def collect_sdk_licenses(output, target, runtime_inputs):
    command = [
        sys.executable, ROOT / "scripts/collect_licenses.py",
        "--package", "bili_core", "--target", target,
        "--output", output / "notices" / "third_party", "--rust-runtime",
    ]
    for path in runtime_inputs:
        command += ["--runtime-license", path]
    run(command, cwd=ROOT)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=MODES, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--profile", choices=("release",), default="release")
    parser.add_argument("--native-prefix", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--runtime-license", type=Path, action="append", default=[])
    args = parser.parse_args()
    if args.target.endswith("windows-msvc") != (sys.platform == "win32"):
        parser.error("build on the target operating system; cross linking is not supported")
    if args.target.endswith("windows-msvc") and not os.environ.get("VCToolsVersion"):
        parser.error("Windows SDK metadata requires the active VCToolsVersion CRT identity")

    prefix = (args.native_prefix or ROOT / "target/native" / args.target).resolve()
    native = native_info(prefix)
    if native.get("target") != args.target:
        raise RuntimeError("native FFmpeg prefix target does not match the requested SDK target")
    output = (args.output or ROOT / "target/sdk" / args.target / args.mode).resolve()
    if output.exists() or output.is_symlink():
        raise RuntimeError(f"refusing to overwrite existing SDK output: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix=f".{output.name}.stage-", dir=output.parent) as temporary:
        stage = Path(temporary)
        package_notices(stage, prefix)
        if args.mode == "dylib":
            copy_native_sdk(prefix, stage, args.target)
        core = None
        if args.mode == "rlib":
            executable, env, consumer_lock, artifacts, reachable, import_inputs = build_source_consumer(
                stage, prefix, args.target, args.profile
            )
            lookup_prefix = stage / "native"
            lookup = dependency_lookup_path(env, stage, lookup_prefix, args.target)
            inspection = inspect_rlib(executable, stage, args.target, lookup)
        else:
            executable, core, artifacts, reachable, env, consumer_lock, import_inputs = build_dynamic_consumer(
                stage, prefix, args.target, args.profile
            )
            lookup_prefix = stage / "native"
            lookup = dependency_lookup_path(env, stage, lookup_prefix, args.target)
            inspection = inspect_dynamic(executable, core, stage, args.target, lookup)
        if args.target.endswith("windows-msvc"):
            compiler_inputs = {path.name for path in compiler_artifact_paths(artifacts, reachable)}
            for library in (stage / "lib").glob("*.dll"):
                if library.name not in compiler_inputs:
                    library.unlink()

        runpaths = inspect_runtime_search_paths(stage)
        if not args.target.endswith("windows-msvc"):
            finalize_elf_inspection(inspection, executable, core, args.mode, runpaths)
        runtime_env = isolated_runtime_env(env)
        with tempfile.TemporaryDirectory(prefix="bili-sdk-state-") as temporary_state:
            state = Path(temporary_state) / "state"
            result = Path(temporary_state) / "output"
            state.mkdir(mode=0o700)
            result.mkdir(mode=0o700)
            run([executable, state, result], env=runtime_env)
            if args.mode == "dylib":
                core_hash = sha256(core)
                hidden = core.with_name(core.name + ".disabled")
                core.rename(hidden)
                try:
                    failed = subprocess.run(
                        [str(executable), str(state), str(result)],
                        env=runtime_env, text=True, capture_output=True,
                    )
                    if failed.returncode == 0:
                        raise RuntimeError("dylib consumer still ran after its core library was removed")
                finally:
                    hidden.rename(core)
                if sha256(core) != core_hash:
                    raise RuntimeError("restored core library hash changed")
                if not args.target.endswith("windows-msvc") and (
                    core.name not in failed.stderr or "error while loading shared libraries" not in failed.stderr
                ):
                    raise RuntimeError("negative loader control did not identify the missing core library")
                restored = run([executable, state, result], env=runtime_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                inspection["core_removal_control"] = {
                    "removed_library": core.name,
                    "failed_exit_code": failed.returncode,
                    "failed_stdout": failed.stdout,
                    "failed_stderr": failed.stderr,
                    "restored_sha256": core_hash,
                    "restored_healthy_exit_code": restored.returncode,
                }

        runtime_inputs = runtime_license_inputs(inspection, prefix, args.target, args.runtime_license)
        if args.target.endswith("windows-msvc") and any(
            name.lower().startswith(("vcruntime", "msvcp", "concrt"))
            for name in inspection.get("dependency_closure", {})
        ):
            from windows_redist import collect_windows_redist_notices

            redist_notices = stage / "licenses" / "windows-redist"
            runtime_inputs.extend(collect_windows_redist_notices(redist_notices))
        collect_sdk_licenses(stage, args.target, runtime_inputs)
        if args.mode == "rlib":
            shutil.copy2(ROOT / "Cargo.lock", stage / "source" / "Cargo.lock")
        source_cargo = verify_source_cargo(stage, prefix, args.target, env) if args.mode == "rlib" else None

        source_hash = source_digest()
        native_record = {
            "schema_version": native.get("schema_version"),
            "target": native.get("target"),
            "source": native.get("source"),
            "configure": [item for item in native.get("configure", []) if not item.startswith("--prefix=")],
            "compiler": native.get("compiler"),
            "recipe_sha256": native.get("recipe_sha256"),
            "license": native.get("license"),
            "patches": native.get("patches"),
        }
        if args.mode == "rlib":
            core_artifacts = [
                Path(filename).name
                for artifact in artifacts if artifact["target"]["name"] == "bili_core"
                for filename in artifact["filenames"] if Path(filename).suffix == ".rlib"
            ]
            if len(core_artifacts) != 1:
                raise RuntimeError("SDK metadata requires exactly one selected bili_core rlib artifact")
            core_relative = Path("lib") / core_artifacts[0]
        else:
            core_relative = core.relative_to(stage)
        native_build_search = {
            "ffmpeg_dir": "native",
            "include": "native/include",
            "pkg_config_libdir": "native/lib/pkgconfig",
            "library_search": "native/lib",
        }
        runtime_layout = {
            "consumer_executable": str(executable.relative_to(stage)),
            "core_artifact": core_relative.as_posix(),
            "runtime_library_directory": "bin" if args.target.endswith("windows-msvc") else "lib",
            "native_build_search": native_build_search,
            "windows_dll_directory": "bin" if args.target.endswith("windows-msvc") else None,
            "linux_executable_runpath": "$ORIGIN/../lib" if not args.target.endswith("windows-msvc") else None,
            "linux_shared_library_runpath": "$ORIGIN" if not args.target.endswith("windows-msvc") else None,
        }
        metadata = {
            "schema_version": 1,
            "mode": args.mode,
            "source_cargo_verification": source_cargo,
            "target": args.target,
            "target_cpu_features": [flag for flag in shlex.split(env.get("RUSTFLAGS", "")) if "target-cpu" in flag or "target-feature" in flag],
            "profile": args.profile,
            "profile_settings": {
                "opt-level": os.environ.get("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3"),
                "lto": {"false": "off", "true": "fat"}.get(
                    os.environ.get("CARGO_PROFILE_RELEASE_LTO", "false").lower(),
                    os.environ.get("CARGO_PROFILE_RELEASE_LTO", "false").lower(),
                ),
                "codegen-units": os.environ.get("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16"),
                "debug": os.environ.get("CARGO_PROFILE_RELEASE_DEBUG", "false"),
                "debug-assertions": os.environ.get("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "false"),
                "overflow-checks": os.environ.get("CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "false"),
                "strip": {"false": "none", "true": "symbols"}.get(
                    os.environ.get("CARGO_PROFILE_RELEASE_STRIP", "none").lower(),
                    os.environ.get("CARGO_PROFILE_RELEASE_STRIP", "none").lower(),
                ),
                "panic": "unwind",
            },
            "rustc_vv": rust_vv(),
            "workspace_cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
            "consumer_cargo_lock_sha256": sha256(consumer_lock),
            "core_version": core_version(),
            "source_digest": source_hash,
            "source_revision": "working-tree:" + source_hash,
            "dependencies": dependency_versions(args.target),
            "features": core_features(args.target),
            "rustflags": env.get("RUSTFLAGS", ""),
            "panic": "unwind",
            "allocator": "system",
            "native": native_record,
            "native_crt_version": os.environ.get("VCToolsVersion") if args.target.endswith("windows-msvc") else None,
            "registry_import_libraries": import_inputs,
            "native_build_search": native_build_search,
            "runtime_layout": runtime_layout,
            "native_headers": ffmpeg_headers(prefix),
            "consumer_executable": str(executable.relative_to(stage)),
            "inspection": inspection,
            "artifacts": [
                {"path": str(path.relative_to(stage)), "sha256": sha256(path)}
                for path in sorted(stage.rglob("*")) if path.is_file()
            ],
            "compatibility": "Exact build-tuple Rust linkage only; no stable Rust ABI, C ABI, hotloading, or unload support.",
        }
        (stage / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        if output.exists() or output.is_symlink():
            raise RuntimeError(f"SDK output appeared during build: {output}")
        stage.rename(output)
    print(f"Verified {args.mode} SDK: {output}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.CalledProcessError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"SDK verification failed: {error}", file=sys.stderr)
        sys.exit(1)
