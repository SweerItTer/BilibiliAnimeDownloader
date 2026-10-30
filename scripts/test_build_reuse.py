import json
from pathlib import Path
import tempfile
import unittest

from build_ffmpeg import build_fingerprint, installed_files, native_cache_valid
from generate_media_fixtures import corpus_cache_valid, corpus_outputs, corpus_tool_manifest, sha256


class NativeInstallReuseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.prefix = Path(self.temporary.name) / "native"
        self.prefix.mkdir()
        for name in (
            "config.h", "config_components.h", "config.mak", "COPYING.LGPLv2.1", "LICENSE.md",
            "lib/libavformat.so.62", "lib/libavcodec.so.62", "lib/libavutil.so.60",
        ):
            path = self.prefix / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        self.expected = {
            "schema_version": 1,
            "target": "x86_64-unknown-linux-gnu",
            "source": {"version": "8.0.3", "sha256": "a" * 64},
            "prefix": str(self.prefix),
            "configure": ["--prefix=" + str(self.prefix), "--enable-shared"],
            "compiler": "controlled compiler version",
            "recipe_sha256": "b" * 64,
            "license": "LGPL-2.1-or-later",
            "patches": [],
            "build_fingerprint": "c" * 64,
        }
        info = {
            **self.expected,
            "source_archive": "cached archive",
            "import_libraries": [],
            "qualification": "candidate",
            "installed_files": installed_files(self.prefix),
        }
        (self.prefix / "build-info.json").write_text(json.dumps(info), encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def test_only_matching_fingerprint_and_intact_install_is_reusable(self):
        self.assertTrue(native_cache_valid(self.prefix, self.expected))
        self.assertFalse(native_cache_valid(self.prefix, {**self.expected, "build_fingerprint": "d" * 64}))
        (self.prefix / "lib/libavcodec.so.62").write_bytes(b"corrupt")
        self.assertFalse(native_cache_valid(self.prefix, self.expected))

    def test_fingerprint_tracks_build_environment_without_storing_values(self):
        inputs = (
            self.expected["source"], "a" * 64, self.expected["target"], None,
            self.prefix, self.expected["configure"], self.expected["recipe_sha256"],
            {"version_sha256": "e" * 64},
        )
        secret = "private-toolchain-setting"
        before = build_fingerprint(*inputs, {"CFLAGS": secret})
        changed = build_fingerprint(*inputs, {"CFLAGS": secret + "-O2"})
        self.assertNotEqual(before, changed)
        self.assertNotIn(secret, before)


class FixtureCorpusReuseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.output = root / "fixtures"
        self.output.mkdir()
        self.ffmpeg = root / "build/prefix/bin/ffmpeg"
        self.ffmpeg.parent.mkdir(parents=True)
        self.ffmpeg.write_bytes(b"controlled test ffmpeg")
        (self.output / "synthetic.mp4").write_bytes(b"locked synthetic fixture")
        (self.output / "segments.ffconcat").write_text("fixture list\n", encoding="utf-8")
        segment_records = []
        artifact_names = ["synthetic.mp4", "segments.ffconcat"]
        for index in (1, 2):
            name = f"segment-0{index}.flv"
            video_oracle = name.replace(".flv", ".video.framemd5")
            audio_oracle = name.replace(".flv", ".audio.framemd5")
            for filename, contents in ((name, f"segment {index}".encode()),
                                       (video_oracle, b"video oracle"),
                                       (audio_oracle, b"audio oracle")):
                (self.output / filename).write_bytes(contents)
                artifact_names.append(filename)
            segment_records.append({
                "path": name,
                "sha256": sha256(self.output / name),
                "decoded_video_oracle_sha256": sha256(self.output / video_oracle),
                "decoded_audio_oracle_sha256": sha256(self.output / audio_oracle),
            })
        manifest = {
            "schema_version": 1,
            "synthetic_only": True,
            "provenance": {"fingerprint": "f" * 64, "ffmpeg_sha256": sha256(self.ffmpeg)},
            "fixtures": [{"path": "synthetic.mp4", "sha256": sha256(self.output / "synthetic.mp4"),
                          "bytes": (self.output / "synthetic.mp4").stat().st_size}],
            "sequential_flv": {"concat_list": "segments.ffconcat", "segments": segment_records},
        }
        manifest["artifacts"] = corpus_outputs(self.output, artifact_names)
        (self.output / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def test_only_locked_provenance_and_intact_outputs_are_reusable(self):
        fingerprint = "f" * 64
        self.assertTrue(corpus_cache_valid(self.output, fingerprint, self.ffmpeg))
        self.assertFalse(corpus_cache_valid(self.output, "0" * 64, self.ffmpeg))
        (self.output / "user-note.txt").write_text("unowned output", encoding="utf-8")
        self.assertTrue(corpus_cache_valid(self.output, fingerprint, self.ffmpeg))
        self.assertIsNotNone(corpus_tool_manifest(self.output, fingerprint, self.ffmpeg))
        (self.output / "synthetic.mp4").write_bytes(b"corrupt fixture")
        self.assertFalse(corpus_cache_valid(self.output, fingerprint, self.ffmpeg))
        self.assertIsNotNone(corpus_tool_manifest(self.output, fingerprint, self.ffmpeg))

    def test_corrupt_cached_encoder_invalidates_fixture_reuse(self):
        self.assertTrue(corpus_cache_valid(self.output, "f" * 64, self.ffmpeg))
        self.ffmpeg.write_bytes(b"changed encoder")
        self.assertFalse(corpus_cache_valid(self.output, "f" * 64, self.ffmpeg))


if __name__ == "__main__":
    unittest.main()
