import tempfile
import unittest
from pathlib import Path

from verify_core_linkage import MODES, ROOT, SOURCE_BUILD_ROOT, TARGETS, source_build_target_dir


class SourceConsumerTargetDirectoryTests(unittest.TestCase):
    def test_cache_is_stable_outside_staging_and_published_sdk_roots(self):
        with tempfile.TemporaryDirectory() as temporary:
            sdk_parent = Path(temporary) / "sdk" / TARGETS[0]
            sdk_parent.mkdir(parents=True)
            output_roots = (
                sdk_parent / ".rlib.stage-first",
                sdk_parent / ".rlib.stage-second",
                sdk_parent / "rlib",
            )
            cache = source_build_target_dir(TARGETS[0], "rlib").resolve()
            fallbacks = []
            for output in output_roots:
                with self.subTest(output=output.name):
                    fallback = (output / SOURCE_BUILD_ROOT / TARGETS[0] / "rlib").resolve()
                    fallbacks.append(fallback)
                    self.assertFalse(fallback.is_relative_to(output.resolve()))
                    self.assertFalse(cache.is_relative_to(output.resolve()))
            self.assertEqual(len(set(fallbacks)), 1)

    def test_target_and_producer_caches_are_disjoint(self):
        consumers = {
            source_build_target_dir(target, mode)
            for target in TARGETS
            for mode in MODES
        }
        self.assertEqual(len(consumers), len(TARGETS) * len(MODES))
        for target in TARGETS:
            producer = ROOT / "target" / "sdk-build" / target / "rlib"
            self.assertNotEqual(source_build_target_dir(target, "rlib"), producer)


if __name__ == "__main__":
    unittest.main()
