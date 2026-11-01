from pathlib import Path
import tempfile
import unittest
from unittest import mock

from verify_package import has_forbidden_native_options, inspect_windows, parse_windows_dependencies


class NativeConfigurationPolicyTests(unittest.TestCase):
    options = ("GPL", "NONFREE", "VERSION3", "FFMPEG", "FFPROBE")

    def test_disabled_and_commented_options_are_not_enabled(self):
        text = "\n".join(
            line
            for name in self.options
            for line in (
                f"!CONFIG_{name}=yes",
                f"#define CONFIG_{name} 0",
                f"# CONFIG_{name}=yes",
                f"// #define CONFIG_{name} 1",
                f"/* #define CONFIG_{name} 1 */",
            )
        )
        self.assertFalse(has_forbidden_native_options(text))

    def test_positive_make_and_c_definitions_are_enabled(self):
        for name in self.options:
            for text in (
                f"CONFIG_{name}=yes",
                f"  CONFIG_{name} = yes # enabled",
                f"#define CONFIG_{name} 1",
                f"  # define CONFIG_{name} 1 /* enabled */",
            ):
                with self.subTest(option=name, configuration=text):
                    self.assertTrue(has_forbidden_native_options(text))

    def test_negative_entry_does_not_hide_later_positive(self):
        for name in self.options:
            with self.subTest(option=name):
                self.assertTrue(
                    has_forbidden_native_options(
                        f"!CONFIG_{name}=yes\n#define CONFIG_{name} 0\nCONFIG_{name}=yes\n"
                    )
                )


class WindowsDependencyParsingTests(unittest.TestCase):
    def test_only_bare_dll_names_are_dependencies(self):
        output = (
            "Dump of file D:\\package with spaces\\avcodec-62.dll\n"
            "\n"
            "    KERNEL32.DLL\n"
            "    Unknown-Mixed_1.DlL\n"
        )
        self.assertEqual(
            parse_windows_dependencies(output),
            ["KERNEL32.DLL", "Unknown-Mixed_1.DlL"],
        )

    def test_gui_accepts_windows_common_controls_but_requires_non_system_libraries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ("bili.exe", "avformat-62.dll", "avcodec-62.dll", "avutil-60.dll"):
                (root / name).write_bytes(b"synthetic import inspection input")
            for product, dependency, accepted in (
                ("gui", "COMCTL32.dll", True),
                ("cli", "COMCTL32.dll", False),
                ("gui", "Unknown-Mixed_1.DlL", False),
                ("gui", "WebView2Loader.dll", False),
            ):
                with self.subTest(product=product, dependency=dependency):
                    def imports(command, env, cwd):
                        return f"    {dependency}\n" if Path(command[-1]).name == "bili.exe" else ""

                    with mock.patch("verify_package.shutil.which", return_value="dumpbin"), \
                            mock.patch("verify_package.run", side_effect=imports):
                        if accepted:
                            inspect_windows(root, {}, product=product)
                        else:
                            with self.assertRaisesRegex(ValueError, "unbundled Windows dependency"):
                                inspect_windows(root, {}, product=product)


if __name__ == "__main__":
    unittest.main()
