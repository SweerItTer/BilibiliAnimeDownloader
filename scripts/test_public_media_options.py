import contextlib
import io
import sys
import unittest
from unittest import mock

import verify_package

LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"
SAMPLE = "BV1gD4y1o72p"


class PublicMediaOptionTests(unittest.TestCase):
    def invoke(self, argv):
        with mock.patch.object(sys, "argv", ["verify_package.py", *argv]), \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as raised:
                verify_package.main()
        return raised.exception.code

    def basic_argv(self, **options):
        argv = ["package.tar.gz", "--native-source", "native.tar.gz", "--public-media-profile", "basic"]
        for name, value in options.items():
            argv.extend(["--" + name.replace("_", "-"), value])
        return argv

    def test_invalid_basic_options_exit_two(self):
        cases = {
            "wrong_media_id": {"target": LINUX, "public_media_id": "BV1xx411c7mD", "probe_helper": "probe"},
            "unsupported_target": {"target": WINDOWS, "public_media_id": SAMPLE, "probe_helper": "probe"},
            "missing_probe": {"target": LINUX, "public_media_id": SAMPLE},
            "missing_media_id": {"target": LINUX, "probe_helper": "probe"},
        }
        for case, options in cases.items():
            with self.subTest(case=case):
                self.assertEqual(self.invoke(self.basic_argv(**options)), 2)
