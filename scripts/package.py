#!/usr/bin/env python3
"""Create a reproducible-layout CLI or GUI package with matching source artifacts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from verify_package import has_forbidden_native_options, inspect_linux, inspect_windows, parse_windows_dependencies
from windows_redist import collect_windows_redist_notices

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {"x86_64-unknown-linux-gnu": "linux", "x86_64-pc-windows-msvc": "windows"}


def sha256(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def copy_tree_files(src, dst, predicate):
    count = 0
    for path in sorted(src.rglob("*")):
        if path.is_file() or path.is_symlink():
            if predicate(path):
                target = dst / path.relative_to(src)
                target.parent.mkdir(parents=True, exist_ok=True)
                if path.is_symlink():
                    target.symlink_to(os.readlink(path))
                else:
                    shutil.copy2(path, target)
                count += 1
    return count

def collect_dependency_licenses(stage, work, target, runtime_license_paths=(), product="core-cli"):
    output = work / f"{product}-license-output"
    command = [
        sys.executable, str(ROOT / "scripts/collect_licenses.py"),
        "--package", product, "--target", target, "--output", str(output), "--rust-runtime",
    ]
    for path in runtime_license_paths:
        command.extend(("--runtime-license", str(path)))
    subprocess.run(command, cwd=ROOT, check=True)
    copy_tree_files(output / "licenses", stage / "licenses", lambda _: True)
    shutil.copy2(output / "index.json", stage / "DEPENDENCY_LICENSES.json")


def make_native_source_bundle(stage_root, target, prefix, info_path, archive, lock):
    stage = stage_root / f"native-source-{target}"
    stage.mkdir()
    native = stage / "share/bili/native"
    source = native / "source"
    source.mkdir(parents=True)
    shutil.copy2(info_path, native / "build-info.json")
    for name in ("config.h", "config_components.h", "config.mak", "COPYING.LGPLv2.1", "LICENSE.md"):
        shutil.copy2(prefix / name, native / name)
    shutil.copy2(archive, source / lock["archive"])
    recipe = source / "scripts"
    recipe.mkdir()
    shutil.copy2(ROOT / "scripts/build_ffmpeg.py", recipe / "build_ffmpeg.py")
    shutil.copy2(ROOT / "scripts/native-lock.json", recipe / "native-lock.json")
    shutil.copy2(ROOT / "scripts/THIRD_PARTY_NOTICES.txt", stage / "THIRD_PARTY_NOTICES.txt")
    building = (
        "This bundle contains the exact FFmpeg source archive and build recipe for the matching package.\n"
        "Run commands from share/bili/native/source; the source pin is in scripts/native-lock.json and "
        "the build configuration is in ../build-info.json.\n"
        "Linux x86_64 requires a C compiler, make, NASM, pkg-config, Clang/libclang and patchelf. Run "
        "`python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu --prefix <prefix> --source-cache .`.\n"
        "Windows x86_64 requires MSYS2 Bash/make/NASM and an initialized Visual Studio x64 MSVC "
        "environment. Run `python.exe scripts/build_ffmpeg.py --target x86_64-pc-windows-msvc "
        "--prefix <prefix> --source-cache .`; it uses MSVC, not MinGW.\n"
    )
    (source / "BUILDING.txt").write_text(building, encoding="utf-8")
    sums = [
        f"{sha256(path)}  {path.relative_to(stage).as_posix()}"
        for path in sorted(stage.rglob("*")) if path.is_file()
    ]
    (stage / "SHA256SUMS").write_text("\n".join(sums) + "\n", encoding="ascii")
    result = stage_root / f"bili-native-source-{target}.tar.gz"
    with tarfile.open(result, "w:gz") as tar:
        tar.add(stage, arcname=f"bili-native-source-{target}", recursive=True)
    return result


def make_sdk_archive(sdk, target, mode, output_dir):
    if not sdk.is_dir() or not (sdk / "build-info.json").is_file():
        raise SystemExit(f"missing {mode} SDK output or build metadata: {sdk}")
    archive = output_dir / f"bili-sdk-{mode}-{target}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(sdk, arcname=f"bili-sdk-{mode}-{target}", recursive=True)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--product", choices=("cli", "gui"), default="cli",
                        help="package the CLI or the native GUI")
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--profile", choices=("debug", "release"), required=True)
    parser.add_argument("--native-prefix", type=Path)
    parser.add_argument("--runtime-only", action="store_true",
                        help="create only the product and matching native-source assets; omit Rust SDKs")
    args = parser.parse_args()
    kind = TARGETS[args.target]
    if (kind == "windows") != (sys.platform == "win32") or (kind == "linux") != (sys.platform == "linux"):
        parser.error("package on the matching target operating system")
    prefix = (args.native_prefix or ROOT / "target/native" / args.target).resolve()
    info_path = prefix / "build-info.json"
    if not info_path.is_file():
        raise SystemExit(f"missing native build metadata: {info_path}")
    native = json.loads(info_path.read_text(encoding="utf-8"))
    lock = json.loads((ROOT / "scripts/native-lock.json").read_text(encoding="utf-8"))
    recipe = ROOT / "scripts/build_ffmpeg.py"
    archive = Path(native.get("source_archive", ""))
    if not archive.is_file():
        archive = ROOT / "target/native/sources" / lock["archive"]
    if not archive.is_file() or sha256(archive) != lock["sha256"]:
        raise SystemExit("pinned FFmpeg source archive is missing or has the wrong SHA-256")
    if native.get("target") != args.target or native.get("source") != lock:
        raise SystemExit("native build metadata target/source pin does not match this package")
    if native.get("patches") != []:
        raise SystemExit("native build recorded patches that this package does not bundle")
    if native.get("source", {}).get("version") != "8.0.3" or native.get("license") != "LGPL-2.1-or-later":
        raise SystemExit("native build metadata has an unacceptable FFmpeg version/license")
    effective = native.get("configure", [])
    if not effective or not all(flag in effective for flag in lock["configure"]):
        raise SystemExit("effective FFmpeg configure flags do not include the pinned configuration")
    for name in ("config.h", "config_components.h", "config.mak"):
        config = prefix / name
        if not config.is_file():
            raise SystemExit(f"missing effective FFmpeg build configuration: {config}")
        text = config.read_text(encoding="utf-8", errors="replace")
        if has_forbidden_native_options(text):
            raise SystemExit(f"forbidden FFmpeg option enabled in {name}")
    forbidden = ("--enable-gpl", "--enable-nonfree", "--enable-version3", "--enable-static", "--enable-programs")
    if any(flag == item or flag.startswith(item + "=") for flag in effective for item in forbidden):
        raise SystemExit("native build enables a forbidden GPL/nonfree/v3/static/program component")
    if sha256(recipe) != native.get("recipe_sha256"):
        raise SystemExit("native build recipe changed since the recorded build")
    gui = args.product == "gui"
    binary = ("gui.exe" if kind == "windows" else "gui") if gui else ("bili.exe" if kind == "windows" else "bili")
    app = ROOT / "target" / args.target / args.profile / binary
    if not app.is_file():
        raise SystemExit(f"missing built {'GUI' if gui else 'CLI'} executable: {app}")
    out = ROOT / "target/packages" / args.target
    if gui:
        out = out / "gui"
    out.mkdir(parents=True, exist_ok=True)
    package_work = tempfile.TemporaryDirectory(prefix=".bili-package-", dir=out)
    work = Path(package_work.name)
    stem = (
        f"bili-gui-{args.target}-{args.profile}"
        if gui
        else f"bili-{args.target}-{args.profile}"
    )
    stage = work / stem
    stage.mkdir()
    native_source_asset = make_native_source_bundle(work, args.target, prefix, info_path, archive, lock)
    runtime_license_paths = []
    crt_payload_files = []
    if kind == "windows":
        package_app = stage / binary
        shutil.copy2(app, package_app)
        dlls = sorted(p for p in prefix.rglob("*") if p.is_file() and p.suffix.lower() == ".dll")
        for dll in dlls:
            destination = stage / dll.name
            if destination.exists():
                raise SystemExit(f"duplicate native DLL basename: {dll.name}")
            shutil.copy2(dll, destination)
        names = {path.name.lower() for path in stage.glob("*.dll")}
        required = ({"avformat-62.dll", "libavformat-62.dll"}, {"avcodec-62.dll", "libavcodec-62.dll"}, {"avutil-60.dll", "libavutil-60.dll"})
        if not all(names & variants for variants in required):
            raise SystemExit("native prefix lacks the required FFmpeg shared-library DLL closure")
        dumpbin = shutil.which("dumpbin")
        redist = os.environ.get("VCToolsRedistDir")
        if not dumpbin or not redist:
            raise SystemExit("Windows packaging requires dumpbin and the active MSVC VCToolsRedistDir")
        deps = set()
        for binary in [package_app, *stage.glob("*.dll")]:
            output = subprocess.run([dumpbin, "/nologo", "/dependents", str(binary)], check=True, capture_output=True, text=True).stdout
            deps.update(name.lower() for name in parse_windows_dependencies(output))
        bundled = {p.name.lower() for p in stage.glob("*.dll")}
        crt_names = {name for name in deps if name.startswith(("vcruntime", "msvcp", "concrt"))}
        missing_crt = crt_names - bundled
        if missing_crt:
            roots = Path(redist).rglob("Microsoft.VC143.CRT")
            crt = next((path for path in roots if "x64" in str(path).lower() and all((path / name).is_file() for name in missing_crt)), None)
            if not crt:
                raise SystemExit(f"cannot locate complete x64 MSVC CRT closure: {sorted(missing_crt)}")
            for dll in crt.glob("*.dll"):
                destination = stage / dll.name
                if destination.exists():
                    raise SystemExit(f"duplicate MSVC runtime DLL basename: {dll.name}")
                shutil.copy2(dll, destination)
            bundled = {p.name.lower() for p in stage.glob("*.dll")}
            if not missing_crt.issubset(bundled):
                raise SystemExit(f"copied MSVC runtime does not satisfy imported DLLs: {sorted(missing_crt - bundled)}")
        crt_payload_files = sorted(
            path for path in stage.glob("*.dll")
            if path.name.lower().startswith(("vcruntime", "msvcp", "concrt"))
        )
        inspect_windows(stage, os.environ.copy(), binary)
        if crt_payload_files:
            runtime_license_paths = collect_windows_redist_notices(stage / "licenses/windows-redist")
    else:
        (stage / "bin").mkdir()
        package_app = stage / "bin" / binary
        staged_app = package_app
        shutil.copy2(app, staged_app)
        staged_app.chmod(0o755)
        patchelf = shutil.which("patchelf")
        if not patchelf:
            raise SystemExit("patchelf is required to set package-relative Linux RUNPATH")
        subprocess.run([patchelf, "--set-rpath", "$ORIGIN/../lib", str(staged_app)], check=True)
        runpath = subprocess.run(["readelf", "-d", str(staged_app)], check=True, capture_output=True, text=True).stdout
        if "$ORIGIN/../lib" not in runpath:
            raise SystemExit("failed to set app-relative Linux RUNPATH")
        lib = prefix / "lib"
        count = copy_tree_files(lib, stage / "lib", lambda p: ".so" in p.name)
        required = ("libavformat.so.62", "libavcodec.so.62", "libavutil.so.60")
        if not count or not all(any((stage / "lib").glob(name + "*")) for name in required):
            raise SystemExit("native prefix lacks the required FFmpeg shared-library closure")
        for native_lib in sorted((stage / "lib").glob("*.so*")):
            if not native_lib.is_symlink():
                subprocess.run([patchelf, "--set-rpath", "$ORIGIN", str(native_lib)], check=True)
        inspect_linux(stage, {"PATH": "/usr/bin:/bin"})
    collect_dependency_licenses(stage, work, args.target, runtime_license_paths,
                                "gui" if gui else "core-cli")
    native_dir = stage / "share/bili/native"
    native_dir.mkdir(parents=True)
    shutil.copy2(info_path, native_dir / "build-info.json")
    shutil.copy2(prefix / "config.h", native_dir / "config.h")
    shutil.copy2(prefix / "config_components.h", native_dir / "config_components.h")
    shutil.copy2(prefix / "config.mak", native_dir / "config.mak")
    for name in ("COPYING.LGPLv2.1", "LICENSE.md"):
        file = prefix / name
        if not file.is_file():
            raise SystemExit(f"missing native license/source notice: {file}")
        shutil.copy2(file, native_dir / name)
    source_dir = native_dir / "source"
    source_dir.mkdir()
    shutil.copy2(ROOT / "scripts/native-lock.json", source_dir / "native-lock.json")
    (source_dir / "BUILDING.txt").write_text(
        f"Obtain {native_source_asset.name} beside the runtime package and verify it with the adjacent SHA256SUMS. "
        "It contains the exact matching FFmpeg source, build recipe, configuration and license notices. "
        "See FFMPEG_SOURCE.txt for the pinned source archive details.\n",
        encoding="utf-8")
    (source_dir / "FFMPEG_SOURCE.txt").write_text(
        f"source_asset={native_source_asset.name}\n"
        f"asset_sha256={sha256(native_source_asset)}\n"
        f"ffmpeg_archive={lock['archive']}\n"
        f"ffmpeg_sha256={lock['sha256']}\n",
        encoding="utf-8")
    shutil.copy2(ROOT / "LICENSE", stage / "LICENSE")
    shutil.copy2(ROOT / "scripts/THIRD_PARTY_NOTICES.txt", stage / "THIRD_PARTY_NOTICES.txt")
    usage = (
        "Bilibili GUI package\n\nRun the gui executable to open the desktop window. Keep Linux bin/ and lib/ together; "
        "on Windows keep gui.exe beside its runtime DLLs.\n"
        if gui
        else "Bilibili CLI package\n\nRun bili --help for commands. Keep Linux bin/ and lib/ together; "
        "on Windows keep bili.exe beside its runtime DLLs.\n"
    )
    usage += (
        "The application license is in LICENSE. FFmpeg is a separate replaceable LGPL shared library, "
        "not an executable; see THIRD_PARTY_NOTICES.txt and share/bili/native/source for source and "
        "replacement instructions.\n"
        "Dependency license texts are under licenses/ and indexed by DEPENDENCY_LICENSES.json.\n"
    )
    if crt_payload_files:
        usage += (
            "This package includes Microsoft Visual C++ runtime files governed by separate Microsoft terms. "
            "Recipients must agree to the applicable protective terms; read the supplied documents and provenance "
            "under licenses/windows-redist/ and the Windows section of THIRD_PARTY_NOTICES.txt.\n"
        )
    (stage / "USAGE.txt").write_text(usage, encoding="utf-8")
    rustc = subprocess.run(["rustc", "-Vv"], check=True, capture_output=True, text=True).stdout
    version_output = subprocess.run([str(package_app), "--version"], check=True,
                                    capture_output=True, text=True).stdout.strip()
    prefix = "Bili " if gui else "bili "
    if not version_output.startswith(prefix):
        raise SystemExit(f"cannot determine packaged version from: {version_output!r}")
    app_version = version_output.removeprefix(prefix)
    metadata = {
        "schema_version": 1, "product": "gui" if gui else "cli",
        "target": args.target, "profile": args.profile,
        "app_version": app_version,
        "rustc": rustc.strip(), "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
        "native_build_info_sha256": sha256(info_path), "native_configure": effective,
        "app_sha256": sha256(package_app), "native_prefix_not_runtime_path": True,
    }
    if kind == "windows":
        toolset_version = os.environ.get("VCToolsVersion")
        if not toolset_version:
            raise SystemExit("Windows package must record the active MSVC toolset version")
        metadata["windows_msvc_toolset_version"] = toolset_version
        metadata["windows_sdk_version"] = os.environ.get("WindowsSDKVersion")
        metadata["windows_vc_redist_directory"] = Path(os.environ["VCToolsRedistDir"]).name
        metadata["windows_vc_runtime_dlls"] = [
            {"name": path.name, "sha256": sha256(path)} for path in crt_payload_files
        ]
    (stage / "BUILD_INFO.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    sums = []
    for path in sorted(p for p in stage.rglob("*") if p.is_file() and p.name != "SHA256SUMS"):
        sums.append(f"{sha256(path)}  {path.relative_to(stage).as_posix()}")
    (stage / "SHA256SUMS").write_text("\n".join(sums) + "\n", encoding="ascii")
    package = work / (stem + (".zip" if kind == "windows" else ".tar.gz"))
    if kind == "windows":
        with zipfile.ZipFile(package, "w", zipfile.ZIP_DEFLATED) as z:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    z.write(path, (Path(stem) / path.relative_to(stage)).as_posix())
    else:
        with tarfile.open(package, "w:gz") as tar:
            tar.add(stage, arcname=stem, recursive=True)
    sdk_archives = []
    if not args.runtime_only:
        sdk_root = ROOT / "target/sdk" / args.target
        if not sdk_root.is_dir():
            raise SystemExit(f"missing matched SDK output: {sdk_root}")
        for mode in ("rlib", "dylib"):
            sdk_archives.append(make_sdk_archive(sdk_root / mode, args.target, mode, work))
    descriptor_asset = None
    if gui:
        descriptor = {
            "schema_version": 1,
            "product": "gui",
            "target": args.target,
            "version": app_version,
            "archive": {
                "name": package.name,
                "format": "zip" if kind == "windows" else "tar.gz",
                "sha256": sha256(package),
                "size": package.stat().st_size,
            },
        }
        descriptor_asset = work / f"bili-update-gui-{args.target}.json"
        descriptor_asset.write_text(json.dumps(descriptor, indent=2) + "\n", encoding="utf-8")
    extras = [path for path in (descriptor_asset, *sdk_archives) if path is not None]
    external_sums = [f"{sha256(package)}  {package.name}", f"{sha256(native_source_asset)}  {native_source_asset.name}"] + [
        f"{sha256(path)}  {path.name}" for path in extras
    ]
    sums_file = work / "SHA256SUMS"
    sums_file.write_text("\n".join(external_sums) + "\n", encoding="ascii")
    for artifact in (native_source_asset, package, *extras, sums_file):
        os.replace(artifact, out / artifact.name)
    package = out / package.name
    native_source_asset = out / native_source_asset.name
    descriptor_asset = out / descriptor_asset.name if descriptor_asset is not None else None
    sdk_archives = [out / path.name for path in sdk_archives]
    package_work.cleanup()
    print(f"Package: {package}")
    print(f"Native source: {native_source_asset}")
    if descriptor_asset is not None:
        print(f"Update descriptor: {descriptor_asset}")
    for path in sdk_archives:
        print(f"SDK: {path}")


if __name__ == "__main__":
    main()
