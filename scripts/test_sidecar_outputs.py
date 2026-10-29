import hashlib
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

from verify_package import validate_sidecar_outputs


class SidecarOutputTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.output = self.root / "output"
        self.output.mkdir()
        self.texts = {
            "danmaku-raw": "",
            "danmaku-xml": '<i><chatserver>chat.bilibili.com</chatserver><chatid>0</chatid><mission>0</mission><maxlimit>0</maxlimit><state>0</state><real_name>0</real_name><source>k-v</source></i>',
            "danmaku-ass": "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize\nStyle: Default,Arial,40\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        }
        self.components = []
        for name, text in self.texts.items():
            path = self.output / name
            path.write_text(text, encoding="utf-8")
            self.components.append({"name": name, "path": str(path), "error": None, "omissions": 0})

    def test_valid_empty_pool_retains_file_and_digest_proofs(self):
        results = validate_sidecar_outputs(self.components, self.output)
        self.assertEqual([item["count"] for item in results], [0, 0, 0])
        for item, component in zip(results, self.components):
            self.assertEqual(item["sha256"], hashlib.sha256(Path(component["path"]).read_bytes()).hexdigest())

    def test_malformed_and_wrong_formats_are_rejected(self):
        for name, text in (("danmaku-raw", "not JSON\n"), ("danmaku-raw", '{"unexpected": 1}\n'), ("danmaku-xml", "<broken>"), ("danmaku-xml", "<other/>"), ("danmaku-ass", "[Events]\nFormat: Text\n")):
            with self.subTest(name=name, text=text):
                path = self.output / name
                path.write_text(text, encoding="utf-8")
                with self.assertRaises((ValueError, ET.ParseError)):
                    validate_sidecar_outputs(self.components, self.output)
                path.write_text(self.texts[name], encoding="utf-8")

    def test_outside_paths_and_disagreeing_counts_are_rejected(self):
        outside = self.root / "outside.jsonl"
        outside.write_text("", encoding="utf-8")
        changed = [dict(component) for component in self.components]
        changed[0]["path"] = str(outside)
        with self.assertRaises(ValueError):
            validate_sidecar_outputs(changed, self.output)
        xml = self.output / "danmaku-xml"
        xml.write_text(self.texts["danmaku-xml"].replace("</i>", '<d p="0,1,25,16777215,0,0,user,id">hello</d></i>'), encoding="utf-8")
        with self.assertRaises(ValueError):
            validate_sidecar_outputs(self.components, self.output)


if __name__ == "__main__":
    unittest.main()
