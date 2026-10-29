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
    validate_descriptor,
    write_inventory,
)
from verify_package import check_checksums


class PackageMetadataTests(unittest.TestCase):
    target = "x86_64-unknown-linux-gnu"

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


if __name__ == "__main__":
    unittest.main()
