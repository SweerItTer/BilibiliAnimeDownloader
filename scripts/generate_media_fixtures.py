#!/usr/bin/env python3
"""Build an isolated test-only FFmpeg and generate deterministic synthetic remux fixtures."""
import argparse
import hashlib
import json
import os
import shlex
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "scripts/corpus-lock.json"
DEFAULT_CACHE = Path.home() / ".cache/bili-native/corpus-sources"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
CORPUS_BUILD_ENV = (
    "PATH", "CC", "CFLAGS", "CPPFLAGS", "LDFLAGS", "CXX", "CXXFLAGS", "AR", "AS", "LD", "NM",
    "RANLIB", "STRIP", "PKG_CONFIG", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR",
    "PKG_CONFIG_SYSTEM_INCLUDE_PATH", "PKG_CONFIG_SYSTEM_LIBRARY_PATH", "CONFIG_SITE", "MAKE", "MAKEFLAGS",
    "MFLAGS", "CMAKE_PREFIX_PATH", "CMAKE_TOOLCHAIN_FILE", "CMAKE_GENERATOR", "CMAKE_MAKE_PROGRAM",
    "CMAKE_C_COMPILER", "CMAKE_CXX_COMPILER", "CMAKE_C_COMPILER_LAUNCHER", "CMAKE_CXX_COMPILER_LAUNCHER",
    "CMAKE_C_FLAGS", "CMAKE_CXX_FLAGS", "CMAKE_BUILD_TYPE", "CPATH", "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH", "LIBRARY_PATH", "LD_LIBRARY_PATH", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET",
)


def hash_text(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def tool_identity(command, version_args=("--version",)):
    if not command:
        return None
    parts = [command] if Path(command).is_file() else shlex.split(command, posix=os.name != "nt")
    if not parts:
        return None
    tool_files = []
    for part in parts:
        executable = shutil.which(part)
        if executable:
            path = Path(executable).resolve()
            tool_files.append({
                "path_sha256": hash_text(str(path)),
                "binary_sha256": sha256(path) if path.is_file() else None,
            })
    if not tool_files:
        return None
    try:
        version = subprocess.run([*parts, *version_args], capture_output=True, text=True)
        version_sha256 = hash_text(version.stdout + version.stderr)
    except (OSError, subprocess.SubprocessError):
        version_sha256 = None
    return {"command_tools": tool_files, "version_sha256": version_sha256}


def corpus_fingerprint(lock, environment):
    tools = {
        "cc": tool_identity(environment.get("CC", "cc")),
        "cxx": tool_identity(environment.get("CXX", "c++")),
        "cmake": tool_identity("cmake"),
        "ninja": tool_identity("ninja", ("--version",)),
        "make": tool_identity("make"),
        "pkg_config": tool_identity(shutil.which("pkg-config") or shutil.which("pkgconf") or ""),
    }
    inputs = {
        "lock_sha256": hash_text(json.dumps(lock, sort_keys=True, separators=(",", ":"))),
        "recipe_sha256": sha256(Path(__file__).resolve()),
        "environment_sha256": {
            name: hash_text(environment[name]) if name in environment else None
            for name in CORPUS_BUILD_ENV
        },
        "tools": tools,
    }
    encoded = json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def corpus_outputs(output, names):
    files = {}
    for name in names:
        if not isinstance(name, str) or not name or Path(name).name != name or name in (".", ".."):
            raise ValueError("fixture artifact path is not a generated filename")
        path = output / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"fixture artifact is missing or unsafe: {name}")
        files[name] = sha256(path)
    return files


def fixture_artifact_names(manifest):
    names = {item["path"] for item in manifest["fixtures"]}
    sequence = manifest["sequential_flv"]
    names.add(sequence["concat_list"])
    for segment in sequence["segments"]:
        name = segment["path"]
        names.update((name, name.replace(".flv", ".video.framemd5"),
                      name.replace(".flv", ".audio.framemd5")))
    return sorted(names)


def corpus_tool_manifest(output, fingerprint, ffmpeg):
    try:
        manifest_path = output / "manifest.json"
        if (manifest_path.is_symlink() or not manifest_path.is_file()
                or ffmpeg.is_symlink() or not ffmpeg.is_file()):
            return None
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if (manifest.get("schema_version") != 1 or manifest.get("synthetic_only") is not True
                or manifest.get("provenance") != {
                    "fingerprint": fingerprint, "ffmpeg_sha256": sha256(ffmpeg)
                }):
            return None
        return manifest
    except (OSError, ValueError, TypeError, AttributeError, json.JSONDecodeError):
        return None


def corpus_cache_valid(output, fingerprint, ffmpeg):
    manifest = corpus_tool_manifest(output, fingerprint, ffmpeg)
    if manifest is None:
        return False
    try:
        recorded = manifest.get("artifacts")
        if not isinstance(recorded, dict) or not recorded or recorded != corpus_outputs(output, recorded):
            return False
        fixtures = manifest.get("fixtures")
        if not isinstance(fixtures, list) or not fixtures:
            return False
        for item in fixtures:
            path = item.get("path")
            if recorded.get(path) != item.get("sha256"):
                return False
            if (output / path).stat().st_size != item.get("bytes"):
                return False
        sequence = manifest.get("sequential_flv")
        segments = sequence.get("segments") if isinstance(sequence, dict) else None
        if (not isinstance(segments, list) or len(segments) != 2
                or sequence.get("concat_list") not in recorded):
            return False
        for segment in segments:
            if (recorded.get(segment.get("path")) != segment.get("sha256")
                    or recorded.get(segment["path"].replace(".flv", ".video.framemd5"))
                    != segment.get("decoded_video_oracle_sha256")
                    or recorded.get(segment["path"].replace(".flv", ".audio.framemd5"))
                    != segment.get("decoded_audio_oracle_sha256")):
                return False
        return True
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        return False


def validate_generated_paths(work, output, cache):
    home = Path.home()
    for path in (work, output, cache):
        if path == ROOT or path in ROOT.parents or path == home or path in home.parents:
            raise SystemExit(f"refusing unsafe generated-data path: {path}")
        if path.is_relative_to(ROOT) and not path.is_relative_to(ROOT / "target"):
            raise SystemExit(f"generated data inside the repository must stay under target/: {path}")
    paths = (work, output, cache)
    for index, left in enumerate(paths):
        if any(left == right or left in right.parents or right in left.parents
               for right in paths[index + 1:]):
            raise SystemExit("--build-dir, --output-dir, and --source-cache must not overlap")


def clear_corpus_build(work):
    work.mkdir(parents=True, exist_ok=True)
    for name in ("deps", "sources", "build", "prefix"):
        path = work / name
        if path.is_symlink():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)
        elif path.exists():
            path.unlink()


def run(command, cwd=None, env=None):
    subprocess.run(command, cwd=cwd, env=env, check=True)


def fetch(source, cache):
    archive = cache / source["archive"]
    if archive.exists() and sha256(archive) != source["sha256"]:
        archive.unlink()
    if not archive.exists():
        partial = archive.with_suffix(archive.suffix + ".partial")
        urllib.request.urlretrieve(source["url"], partial)
        partial.replace(archive)
    if sha256(archive) != source["sha256"]:
        raise SystemExit(f"source checksum mismatch: {source['name']}")
    return archive


def extract(archive, destination):
    if not destination.exists() or not any(destination.iterdir()):
        destination.mkdir(parents=True, exist_ok=True)
        with tarfile.open(archive) as source:
            source.extractall(destination, filter="data")
    children = [child for child in destination.iterdir() if child.is_dir()]
    if len(children) != 1 or len(list(destination.iterdir())) != 1:
        raise SystemExit(f"unexpected archive layout: {archive.name}")
    return children[0]


def build_encoder_sources(lock, work, cache, jobs):
    prefix = work / "deps"
    prefix.mkdir(parents=True, exist_ok=True)
    for source in lock["encoder_sources"]:
        tree = extract(fetch(source, cache), work / "sources" / source["name"])
        build = work / "build" / source["name"]
        build.mkdir(parents=True, exist_ok=True)
        if source["name"] == "x264":
            run([str(tree / "configure"), "--prefix=" + str(prefix), "--enable-static", "--enable-pic", "--disable-cli", "--disable-opencl", "--disable-asm"], cwd=build)
            run(["make", "-j" + str(jobs)], cwd=build)
            run(["make", "install"], cwd=build)
        elif source["name"] == "x265":
            cmake = tree / "source" / "CMakeLists.txt"
            if not cmake.exists():
                cmake = tree / "CMakeLists.txt"
            cmake_text = cmake.read_text(encoding="utf-8")
            cmake_text = cmake_text.replace("cmake_policy(SET CMP0025 OLD)", "cmake_policy(SET CMP0025 NEW)").replace("cmake_policy(SET CMP0054 OLD)", "cmake_policy(SET CMP0054 NEW)")
            cmake.write_text(cmake_text, encoding="utf-8")
            run(["cmake", "-S", str(cmake.parent), "-B", str(build), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_INSTALL_PREFIX=" + str(prefix), "-DCMAKE_POLICY_VERSION_MINIMUM=3.5", "-DENABLE_SHARED=OFF", "-DENABLE_CLI=OFF", "-DENABLE_ASSEMBLY=OFF", "-DHIGH_BIT_DEPTH=ON", "-DMAIN12=OFF"])
            run(["cmake", "--build", str(build), "--parallel", str(jobs)])
            run(["cmake", "--install", str(build)])
        elif source["name"] == "libaom":
            run(["cmake", "-S", str(tree), "-B", str(build), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_INSTALL_PREFIX=" + str(prefix), "-DBUILD_SHARED_LIBS=OFF", "-DENABLE_TESTS=OFF", "-DENABLE_EXAMPLES=OFF", "-DENABLE_TOOLS=OFF", "-DENABLE_DOCS=OFF", "-DCONFIG_AV1_ENCODER=1", "-DCONFIG_AV1_DECODER=1", "-DAOM_TARGET_CPU=generic", "-DENABLE_NASM=OFF"])
            run(["cmake", "--build", str(build), "--parallel", str(jobs)])
            run(["cmake", "--install", str(build)])
    return prefix


def build_ffmpeg(lock, work, cache, jobs, dep_prefix):
    tree = extract(fetch(lock["ffmpeg"], cache), work / "sources" / "ffmpeg")
    prefix, build = work / "prefix", work / "build" / "ffmpeg"
    build.mkdir(parents=True, exist_ok=True)
    configure = [str(tree / "configure"), "--prefix=" + str(prefix), "--pkg-config-flags=--static", "--disable-everything", "--disable-debug", "--disable-doc", "--disable-ffplay", "--disable-ffprobe", "--disable-network", "--disable-autodetect", "--disable-x86asm", "--disable-shared", "--enable-static", "--enable-gpl", "--enable-ffmpeg", "--enable-avcodec", "--enable-avformat", "--enable-avfilter", "--enable-avdevice", "--enable-swresample", "--enable-libx264", "--enable-libx265", "--enable-libaom", "--enable-encoder=libx264,libx265,libaom_av1,aac,flac,ac3,eac3,rawvideo,pcm_s16le", "--enable-decoder=h264,aac,wrapped_avframe,pcm_s16le", "--enable-demuxer=mov,flv,matroska,concat,lavfi,flac,ac3,eac3", "--enable-muxer=mp4,ipod,flv,matroska,flac,ac3,eac3,adts,framemd5", "--enable-indev=lavfi", "--enable-protocol=file,pipe", "--enable-filter=format,aresample,scale,sine,color,testsrc2,anullsrc", "--enable-parser=h264,hevc,av1,aac,flac,ac3", "--enable-bsf=aac_adtstoasc,extract_extradata,h264_mp4toannexb", "--extra-cflags=-I" + str(dep_prefix / "include"), "--extra-ldflags=-L" + str(dep_prefix / "lib")]
    env = os.environ.copy()
    env["PKG_CONFIG_PATH"] = str(dep_prefix / "lib/pkgconfig")
    env["PKG_CONFIG_LIBDIR"] = env["PKG_CONFIG_PATH"]
    pkg_config = shutil.which("pkg-config") or shutil.which("pkgconf")
    if pkg_config:
        env["PKG_CONFIG"] = pkg_config
        configure.append("--pkg-config=" + pkg_config)
    run(configure, cwd=build, env=env)
    run(["make", "-j" + str(jobs)], cwd=build)
    run(["make", "install"], cwd=build)
    binary = prefix / "bin/ffmpeg"
    binary.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(build / "ffmpeg", binary)
    return binary


def prepare_output(path):
    if path.is_symlink():
        path.unlink()
    elif path.exists() and not path.is_file():
        raise SystemExit(f"fixture output path is not a regular file: {path}")
    return path


def generate(ffmpeg, output, fingerprint):
    output.mkdir(parents=True, exist_ok=True)
    base = [str(ffmpeg), "-hide_banner", "-loglevel", "error", "-nostdin", "-y"]

    def encode(name, input_args, codec_args):
        run([*base, *input_args, *codec_args, str(prepare_output(output / name))])

    testsrc = ["-f", "lavfi", "-i", "testsrc2=size=64x64:rate=12:duration=0.5"]
    tone = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=0.5"]
    encode("h264-aac.mp4", [*testsrc, *tone], ["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-threads", "1", "-c:a", "aac", "-b:a", "32k", "-shortest", "-movflags", "+faststart"])
    for name, color in (("dash-video-1.mp4", "red"), ("dash-video-2.mp4", "blue")):
        encode(name, ["-f", "lavfi", "-i", f"color=c={color}:size=64x64:rate=12:duration=0.25"], ["-an", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-threads", "1"])
    encode("dash-audio.m4a", tone, ["-vn", "-c:a", "aac", "-b:a", "32k"])
    encode("hevc-hdr.mp4", testsrc, ["-c:v", "libx265", "-preset", "ultrafast", "-vf", "scale=iw:ih,format=yuv420p10le", "-pix_fmt", "+yuv420p10le", "-x265-params", "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400", "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc", "-threads", "1"])
    encode("av1.mp4", testsrc, ["-c:v", "libaom-av1", "-cpu-used", "8", "-threads", "1", "-row-mt", "0", "-crf", "40", "-b:v", "0", "-pix_fmt", "yuv420p"])
    encode("legacy.flv", [*testsrc, *tone], ["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-threads", "1", "-c:a", "aac", "-b:a", "32k", "-shortest", "-f", "flv"])
    segments = [("segment-01.flv", "red", 440, 0.4), ("segment-02.flv", "blue", 880, 0.7)]
    for name, color, frequency, duration in segments:
        inputs = ["-f", "lavfi", "-i", f"color=c={color}:size=64x64:rate=12:duration={duration}", "-f", "lavfi", "-i", f"sine=frequency={frequency}:sample_rate=48000:duration={duration}"]
        encode(name, inputs, ["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-threads", "1", "-c:a", "aac", "-b:a", "32k", "-shortest", "-f", "flv"])
    concat_list = prepare_output(output / "segments.ffconcat")
    concat_list.write_text("ffconcat version 1.0\nfile 'segment-01.flv'\nfile 'segment-02.flv'\n", encoding="utf-8")
    remux_output = prepare_output(output / "sequential-flv-remux.mp4")
    run([*base, "-safe", "0", "-f", "concat", "-i", str(concat_list), "-map", "0", "-c", "copy", "-f", "mp4", str(remux_output)])
    for name in ("segment-01.flv", "segment-02.flv"):
        for stream, selector in (("video", "v:0"), ("audio", "a:0")):
            oracle = output / name.replace(".flv", f".{stream}.framemd5")
            run([*base, "-i", str(output / name), "-map", f"0:{selector}", "-f", "framemd5", str(prepare_output(oracle))])
    encode("audio-flac.flac", tone, ["-vn", "-c:a", "flac"])
    encode("audio-ac3.ac3", tone, ["-vn", "-c:a", "ac3", "-b:a", "96k"])
    encode("audio-eac3.eac3", tone, ["-vn", "-c:a", "eac3", "-b:a", "96k"])
    encode("unsupported.mkv", testsrc, ["-an", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-threads", "1"])
    entries = {
        "h264-aac.mp4": ("mp4", ["h264", "aac"], "single-file H.264+AAC"),
        "dash-video-1.mp4": ("mp4", ["h264"], "DASH-like video resource 1"),
        "dash-video-2.mp4": ("mp4", ["h264"], "DASH-like video resource 2"),
        "dash-audio.m4a": ("mp4", ["aac"], "DASH-like audio resource"),
        "hevc-hdr.mp4": ("mp4", ["hevc"], "10-bit BT.2020/PQ, mastering display and MaxCLL"),
        "av1.mp4": ("mp4", ["av1"], "AV1 video"),
        "legacy.flv": ("flv", ["h264", "aac"], "legacy FLV"),
        "segment-01.flv": ("flv", ["h264", "aac"], "ordered segment 1: red video, 440 Hz audio, 0.4 seconds"),
        "segment-02.flv": ("flv", ["h264", "aac"], "ordered segment 2: blue video, 880 Hz audio, 0.7 seconds"),
        "sequential-flv-remux.mp4": ("mp4", ["h264", "aac"], "one video+audio stream pair, 1.1 second ordered timeline"),
        "audio-flac.flac": ("flac", ["flac"], "FLAC audio"),
        "audio-ac3.ac3": ("ac3", ["ac3"], "AC-3 audio"),
        "audio-eac3.eac3": ("eac3", ["eac3"], "E-AC-3 audio"),
        "unsupported.mkv": ("matroska", ["h264"], "unsupported input-container rejection"),
    }
    manifest = {"schema_version": 1, "synthetic_only": True, "generated_by": "scripts/generate_media_fixtures.py", "ffmpeg_version": "8.0.3", "fixtures": []}
    durations = {"segment-01.flv": 0.4, "segment-02.flv": 0.7, "sequential-flv-remux.mp4": 1.1}
    for name, (container, codecs, purpose) in entries.items():
        path = output / name
        video = any(codec in codecs for codec in ("h264", "hevc", "av1"))
        audio = any(codec in codecs for codec in ("aac", "flac", "ac3", "eac3"))
        manifest["fixtures"].append({"path": name, "sha256": sha256(path), "bytes": path.stat().st_size, "container": container, "expected_streams": codecs, "purpose": purpose, "timing": {"expected_duration_seconds": durations.get(name, 0.5), "video_frame_rate": "12/1" if video else None, "audio_sample_rate_hz": 48000 if audio else None, "audio_channels": 1 if audio else None}})
    manifest["sequential_flv"] = {"concat_list": "segments.ffconcat", "expected_order": [segment[0] for segment in segments], "expected_stream_count": 2, "expected_streams": [{"type": "video", "codec": "h264"}, {"type": "audio", "codec": "aac"}], "expected_duration_seconds": 1.1, "segments": [{"path": name, "sha256": sha256(output / name), "duration_seconds": duration, "video_content": color, "audio_tone_hz": frequency, "decoded_video_oracle_sha256": sha256(output / name.replace(".flv", ".video.framemd5")), "decoded_audio_oracle_sha256": sha256(output / name.replace(".flv", ".audio.framemd5"))} for name, color, frequency, duration in segments]}
    manifest["provenance"] = {"fingerprint": fingerprint, "ffmpeg_sha256": sha256(ffmpeg)}
    manifest["artifacts"] = corpus_outputs(output, fixture_artifact_names(manifest))
    prepare_output(output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=ROOT / "target/corpus-build")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/media-fixtures")
    parser.add_argument("--source-cache", type=Path, default=DEFAULT_CACHE)
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 1, 8))
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    work, cache, output = args.build_dir.resolve(), args.source_cache.resolve(), args.output_dir.resolve()
    validate_generated_paths(work, output, cache)
    ffmpeg = work / "prefix/bin/ffmpeg"
    fingerprint = corpus_fingerprint(lock, os.environ)
    tool_manifest = corpus_tool_manifest(output, fingerprint, ffmpeg)
    if tool_manifest is not None and corpus_cache_valid(output, fingerprint, ffmpeg):
        manifest = tool_manifest
    elif tool_manifest is not None:
        manifest = generate(ffmpeg, output, fingerprint)
    else:
        clear_corpus_build(work)
        cache.mkdir(parents=True, exist_ok=True)
        dep_prefix = build_encoder_sources(lock, work, cache, args.jobs)
        ffmpeg = build_ffmpeg(lock, work, cache, args.jobs, dep_prefix)
        manifest = generate(ffmpeg, output, fingerprint)
    print(json.dumps({"ffmpeg": str(ffmpeg), "manifest": str(output / "manifest.json"), "fixtures": [str(output / item["path"]) for item in manifest["fixtures"]]}, indent=2))


if __name__ == "__main__":
    main()
