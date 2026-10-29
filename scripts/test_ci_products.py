import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from stage_ci_artifacts import stage_artifacts


class ProductBundleTests(unittest.TestCase):
    target = "x86_64-unknown-linux-gnu"
    suffix = ".tar.gz"
    library = "bili_core.so"

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.packages = self.root / "target/packages" / self.target
        self.packages.mkdir(parents=True)
        self.names = [f"bili-{self.target}-release{self.suffix}", f"bili-native-source-{self.target}.tar.gz"]
        rows = []
        for name, payload in zip(self.names, (b"runtime archive", b"matching native source")):
            (self.packages / name).write_bytes(payload)
            rows.append(f"{hashlib.sha256(payload).hexdigest()}  {name}")
        (self.packages / "SHA256SUMS").write_text("\n".join(rows) + "\n", encoding="ascii")
        self.sdk = self.root / "target/sdk" / self.target / "dylib"
        self.sdk.mkdir(parents=True)
        (self.sdk / "build-info.json").write_text(json.dumps({"target": self.target, "mode": "dylib"}))
        (self.sdk / "lib").mkdir()
        (self.sdk / "lib" / self.library).write_bytes(b"verified shared SDK")

    def test_product_is_self_consistent_and_sdk_is_separate(self):
        products, inputs = stage_artifacts(self.root, self.target)
        self.assertEqual({path.name for path in products.iterdir()}, {*self.names, "SHA256SUMS"})
        for row in (products / "SHA256SUMS").read_text().splitlines():
            digest, name = row.split("  ", 1)
            self.assertEqual(hashlib.sha256((products / name).read_bytes()).hexdigest(), digest)
        with tarfile.open(inputs / f"bili-sdk-dylib-{self.target}.tar.gz") as archive:
            member = f"bili-sdk-dylib-{self.target}/lib/{self.library}"
            self.assertEqual(archive.extractfile(member).read(), b"verified shared SDK")
        self.assertFalse(any("rlib" in path.name for path in inputs.iterdir()))

    def test_polluted_checksums_or_changed_bytes_publish_nothing(self):
        sums = self.packages / "SHA256SUMS"
        original = sums.read_text()
        for failure in ("sdk-row", "changed-runtime"):
            with self.subTest(failure=failure):
                sums.write_text(original)
                if failure == "sdk-row":
                    sums.write_text(original + f"{'0' * 64}  bili-sdk-rlib.tar.gz\n")
                else:
                    (self.packages / self.names[0]).write_bytes(b"different build")
                with self.assertRaises(SystemExit):
                    stage_artifacts(self.root, self.target)
                self.assertFalse((self.root / "target/ci-products" / self.target).exists())
                self.assertFalse((self.root / "target/ci-inputs" / self.target).exists())

    def test_wrong_sdk_target_does_not_publish_product(self):
        (self.sdk / "build-info.json").write_text(json.dumps({"target": "other-target", "mode": "dylib"}))
        with self.assertRaises(SystemExit):
            stage_artifacts(self.root, self.target)
        self.assertFalse((self.root / "target/ci-products" / self.target).exists())


class WindowsProductBundleTests(ProductBundleTests):
    target = "x86_64-pc-windows-msvc"
    suffix = ".zip"
    library = "bili_core.dll"


if __name__ == "__main__":
    unittest.main()
