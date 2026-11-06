import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import webview2_notices


class WebViewNoticesTests(unittest.TestCase):
    def inputs(self, root):
        crate = root / "registry/webview2-com-sys"
        (crate / "x64").mkdir(parents=True)
        (crate / "Cargo.toml").write_text("[package]\n", encoding="utf-8")
        loader = crate / "x64/WebView2LoaderStatic.lib"
        loader.write_bytes(b"synthetic test SDK input")
        notices = root / "scripts/notices/webview2-sdk"
        notices.mkdir(parents=True)
        (notices / "LICENSE.txt").write_bytes(b"synthetic test notice\r\n")
        digest = lambda data: hashlib.sha256(data).hexdigest()
        lock = {"format": 1, "package": "webview2-com-sys", "crate_version": "test",
                "target": "x86_64-pc-windows-msvc", "version": "test-sdk",
                "url": "https://example.invalid/test-sdk", "sha256": "0" * 64,
                "loader": {"crate_path": "x64/WebView2LoaderStatic.lib", "sha256": digest(loader.read_bytes())},
                "notices": [{"filename": "LICENSE.txt", "sha256": digest((notices / "LICENSE.txt").read_bytes())}]}
        (root / "scripts/webview2-sdk-lock.json").write_text(json.dumps(lock), encoding="utf-8")
        return {"name": "webview2-com-sys", "version": "test", "manifest_path": str(crate / "Cargo.toml")}, loader, notices

    def test_preserves_complete_original_notice_bytes_and_actual_loader_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package, _, notices = self.inputs(root)
            output = root / "out"
            record = webview2_notices.collect(package, "x86_64-pc-windows-msvc", output, root)
            self.assertIsNone(record["spdx"])
            self.assertEqual(record["version"], "test-sdk")
            self.assertEqual((output / record["texts"][0]["path"]).read_bytes(), (notices / "LICENSE.txt").read_bytes())

    def test_rejects_changed_linked_input_changed_notice_or_wrong_dependency(self):
        for failure in ("loader", "notice", "version", "target"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                package, loader, notices = self.inputs(root)
                target = "x86_64-pc-windows-msvc"
                if failure == "loader":
                    loader.write_bytes(b"changed SDK input")
                elif failure == "notice":
                    (notices / "LICENSE.txt").write_bytes(b"trimmed notice")
                elif failure == "version":
                    package["version"] = "unexpected"
                else:
                    target = "x86_64-unknown-linux-gnu"
                with self.assertRaises(ValueError):
                    webview2_notices.collect(package, target, root / "out", root)


if __name__ == "__main__":
    unittest.main()
