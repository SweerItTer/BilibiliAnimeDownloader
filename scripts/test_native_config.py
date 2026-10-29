import unittest

from verify_package import has_forbidden_native_options, parse_windows_dependencies


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


if __name__ == "__main__":
    unittest.main()
