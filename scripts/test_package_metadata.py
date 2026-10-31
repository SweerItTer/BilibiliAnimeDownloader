import hashlib
import os
import json
from pathlib import Path
import tempfile
import unittest

from package_metadata import (
    assemble_install_root,
    current_id,
    descriptor_document,
    installation_payload,
    parse_doctor_identity,
    validate_descriptor,
    write_inventory,
)
from verify_package import check_checksums


class PackageMetadataTests(unittest.TestCase):
    target = "x86_64-unknown-linux-gnu"

    def test_gui_payload_is_versioned_and_cannot_be_opened_as_cli(self):
        target = "x86_64-pc-windows-msvc"
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            payload = base / "payload"
            payload.mkdir()
            (payload / "bili.exe").write_bytes(b"native GUI payload")
            digest = hashlib.sha256(b"native GUI payload").hexdigest()
            (payload / "BUILD_INFO.json").write_text(json.dumps({
                "product": "gui", "app_sha256": digest,
                "update_protocol": 1, "layout_version": 1, "bootstrap_protocol": 1,
            }), encoding="utf-8")
            write_inventory(payload)
            bootstrap = base / "bootstrap.exe"
            bootstrap.write_bytes(b"stable bootstrap")
            root = base / "install"
            assembled = assemble_install_root(root, payload, bootstrap, target, "gui")
            self.assertEqual(installation_payload(root, target, "gui"), assembled)
            self.assertEqual((assembled / "bili.exe").read_bytes(), b"native GUI payload")
            self.assertEqual((root / "bili.exe").read_bytes(), b"stable bootstrap")
            check_checksums(root)
            check_checksums(assembled)
            with self.assertRaises(ValueError):
                installation_payload(root, target, "cli")

    def test_doctor_identity_rejects_cross_product_and_wrong_version(self):
        target = "x86_64-pc-windows-msvc"
        report = {"schema_version": 1, "type": "doctor", "data": {
            "version": "2.0.0", "build_identity": {
                "product": "gui", "target": target, "version": "2.0.0", "update_protocol": 1,
            }, "native": {},
        }}
        text = json.dumps(report)
        self.assertEqual(parse_doctor_identity(text, "bili 2.0.0", target, "gui"), "2.0.0")
        for product, version in (("cli", "2.0.0"), ("gui", "2.0.1")):
            with self.subTest(product=product, version=version), self.assertRaises(ValueError):
                parse_doctor_identity(text, f"bili {version}", target, product)

    def test_versioned_install_root_has_nested_verified_inventories(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            payload = base / "payload"
            (payload / "bin").mkdir(parents=True)
            (payload / "lib").mkdir()
            app = payload / "bin/bili"
            app.write_bytes(b"packaged CLI")
            app_hash = hashlib.sha256(app.read_bytes()).hexdigest()
            library = payload / "lib/libavutil.so.60"
            library.write_bytes(b"runtime library")
            if os.name == "posix":
                (payload / "lib/libavutil.so").symlink_to(library.name)
            (payload / "BUILD_INFO.json").write_text(json.dumps({
                "product": "cli", "app_sha256": app_hash,
                "update_protocol": 1, "layout_version": 1, "bootstrap_protocol": 1,
            }))
            write_inventory(payload)
            bootstrap = base / "bili-bootstrap"
            bootstrap.write_bytes(b"bootstrap")

            root = base / "package"
            assembled = assemble_install_root(root, payload, bootstrap, self.target)
            self.assertEqual(assembled, root / "versions" / f"v-{app_hash[:32]}")
            self.assertEqual(installation_payload(root, self.target), assembled)
            self.assertEqual(current_id((root / "CURRENT").read_bytes()), assembled.name)
            check_checksums(assembled)
            check_checksums(root)
            outer_rows = (root / "SHA256SUMS").read_text(encoding="ascii")
            self.assertIn(f"versions/{assembled.name}/SHA256SUMS", outer_rows)
            self.assertEqual((root / "bin/bili").read_bytes(), b"bootstrap")

    def test_current_pointer_accepts_only_one_version_id(self):
        version_id = "v-0123456789abcdef0123456789abcdef"
        self.assertEqual(current_id(version_id.encode()), version_id)
        self.assertEqual(current_id((version_id + "\r\n").encode()), version_id)
        for raw in (b"../payload", (version_id + "\n\n").encode(), b"V-" + b"0" * 32):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                current_id(raw)

    def test_descriptor_rejects_identity_archive_and_digest_mismatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "bili-x86_64-unknown-linux-gnu-release.tar.gz"
            archive.write_bytes(b"runtime archive")
            descriptor = descriptor_document(self.target, "2.0.0", archive)
            validate_descriptor(descriptor, self.target, "2.0.0", archive)
            for mutate, target, version in (
                (lambda value: value.update(product="gui"), self.target, "2.0.0"),
                (lambda value: value.update(target="other"), self.target, "2.0.0"),
                (lambda value: value.update(version="2.0.1"), self.target, "2.0.0"),
                (lambda value: value["archive"].update(sha256="0" * 64), self.target, "2.0.0"),
                (lambda value: value["archive"].update(size=archive.stat().st_size + 1), self.target, "2.0.0"),
                (lambda value: value["archive"].update(format="zip"), self.target, "2.0.0"),
            ):
                with self.subTest(mutate=mutate):
                    invalid = json.loads(json.dumps(descriptor))
                    mutate(invalid)
                    with self.assertRaises(ValueError):
                        validate_descriptor(invalid, target, version, archive)

    def test_gui_descriptor_is_generalized_and_ignores_unknown_fields(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "bili-gui-x86_64-pc-windows-msvc-release.zip"
            archive.write_bytes(b"gui archive")
            descriptor = descriptor_document(
                "x86_64-pc-windows-msvc", "2.0.0", archive, product="gui")
            descriptor["packaged_at"] = "ignored by older builds"
            descriptor["archive"]["revision"] = 3
            validate_descriptor(
                descriptor, "x86_64-pc-windows-msvc", "2.0.0", archive, product="gui")
            with self.assertRaises(ValueError):
                validate_descriptor(
                    descriptor, "x86_64-pc-windows-msvc", "2.0.0", archive, product="cli")
            for mutate in (
                lambda value: value["archive"].update(name="../escape.tar.gz"),
                lambda value: value["archive"].update(name=".hidden.tar.gz"),
                lambda value: value["archive"].update(size=512 * 1024 * 1024 + 1),
            ):
                invalid = json.loads(json.dumps(descriptor))
                mutate(invalid)
                with self.subTest(mutate=mutate), self.assertRaises(ValueError):
                    validate_descriptor(
                        invalid, "x86_64-pc-windows-msvc", None, product="gui")


if __name__ == "__main__":
    unittest.main()
