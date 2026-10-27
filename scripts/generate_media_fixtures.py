#!/usr/bin/env python3
"""Build an isolated test-only FFmpeg and generate deterministic synthetic remux fixtures."""
import argparse
import hashlib
import json
import os
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


def run(command, cwd=None, env=None):
    subprocess.run(command, cwd=cwd, env=env, check=True)


def fetch(source, cache):
    archive = cache / source["archive"]
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


def generate(ffmpeg, output):
    output.mkdir(parents=True, exist_ok=True)
    base = [str(ffmpeg), "-hide_banner", "-loglevel", "error", "-nostdin", "-y"]

    def encode(name, input_args, codec_args):
        run([*base, *input_args, *codec_args, str(output / name)])

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
    (output / "segments.ffconcat").write_text("ffconcat version 1.0\nfile 'segment-01.flv'\nfile 'segment-02.flv'\n", encoding="utf-8")
    run([*base, "-safe", "0", "-f", "concat", "-i", str(output / "segments.ffconcat"), "-map", "0", "-c", "copy", "-f", "mp4", str(output / "sequential-flv-remux.mp4")])
    for name in ("segment-01.flv", "segment-02.flv"):
        for stream, selector in (("video", "v:0"), ("audio", "a:0")):
            run([*base, "-i", str(output / name), "-map", f"0:{selector}", "-f", "framemd5", str(output / name.replace(".flv", f".{stream}.framemd5"))])
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

    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
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
    work, cache = args.build_dir.resolve(), args.source_cache.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    dep_prefix = build_encoder_sources(lock, work, cache, args.jobs)
    ffmpeg = build_ffmpeg(lock, work, cache, args.jobs, dep_prefix)
    manifest = generate(ffmpeg, args.output_dir.resolve())
    print(json.dumps({"ffmpeg": str(ffmpeg), "manifest": str(args.output_dir.resolve() / "manifest.json"), "fixtures": [str(args.output_dir.resolve() / item["path"]) for item in manifest["fixtures"]]}, indent=2))


if __name__ == "__main__":
    main()
