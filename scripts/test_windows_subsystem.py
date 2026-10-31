import tempfile
import unittest
from pathlib import Path

from verify_package import validate_windows_bootstrap_subsystem


class WindowsBootstrapSubsystemTests(unittest.TestCase):
    @staticmethod
    def image(subsystem):
        pe = 128
        optional_size = 240
        image = bytearray(pe + 24 + optional_size)
        image[:2] = b"MZ"
        image[60:64] = pe.to_bytes(4, "little")
        image[pe:pe + 4] = b"PE\0\0"
        image[pe + 4:pe + 6] = b"\x64\x86"
        image[pe + 20:pe + 22] = optional_size.to_bytes(2, "little")
        image[pe + 24:pe + 26] = b"\x0b\x02"
        image[pe + 24 + 68:pe + 24 + 70] = subsystem.to_bytes(2, "little")
        return image

    def test_gui_bootstrap_has_no_console_and_cli_keeps_its_console(self):
        with tempfile.TemporaryDirectory() as temporary:
            entry = Path(temporary) / "bili.exe"
            for product, subsystem in (("gui", 2), ("cli", 3)):
                entry.write_bytes(self.image(subsystem))
                self.assertEqual(validate_windows_bootstrap_subsystem(entry, product), subsystem)
                with self.assertRaises(ValueError):
                    validate_windows_bootstrap_subsystem(entry, "cli" if product == "gui" else "gui")

    def test_malformed_or_non_x64_executable_cannot_prove_its_subsystem(self):
        images = [self.image(2)[:40], self.image(2)[:-1], bytearray(b"not a PE executable")]
        wrong_signature = self.image(2)
        wrong_signature[128:132] = b"NOPE"
        images.append(wrong_signature)
        wrong_machine = self.image(2)
        wrong_machine[132:134] = b"\x4c\x01"
        images.append(wrong_machine)
        wrong_optional = self.image(2)
        wrong_optional[152:154] = b"\x0b\x01"
        images.append(wrong_optional)
        with tempfile.TemporaryDirectory() as temporary:
            entry = Path(temporary) / "bili.exe"
            for image in images:
                with self.subTest(image=image[:4]):
                    entry.write_bytes(image)
                    with self.assertRaises(ValueError):
                        validate_windows_bootstrap_subsystem(entry, "gui")


if __name__ == "__main__":
    unittest.main()
