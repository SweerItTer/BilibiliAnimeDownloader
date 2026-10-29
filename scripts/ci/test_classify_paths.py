import unittest

from classify_paths import classify_event, classify_paths


class ClassifyPathsTests(unittest.TestCase):
    def test_explicit_documentation_paths_are_lightweight(self):
        result = classify_paths((
            "README.md",
            ".trellis/spec/core-cli/backend/quality-guidelines.md",
            ".github/ISSUE_TEMPLATE/bug_report.yml",
            ".github/pull_request_template.md",
        ))
        self.assertEqual(result["scope"], "docs")
        self.assertTrue(result["docs_changed"])
        self.assertFalse(result["run_cli"])

    def test_cli_only_and_gui_only_routes(self):
        self.assertEqual(classify_paths(("core-cli/src/main.rs",))["scope"], "cli")
        self.assertEqual(classify_paths(("gui/src/main.rs",))["scope"], "gui")

    def test_documentation_does_not_widen_a_single_package_route(self):
        self.assertEqual(classify_paths(("README.md", "gui/src/main.rs"))["scope"], "gui")
        self.assertEqual(classify_paths(("README.md", "core-cli/src/main.rs"))["scope"], "cli")

    def test_shared_unknown_or_mixed_package_paths_require_full_checks(self):
        for path in (
            "bili_core/src/lib.rs",
            "Cargo.toml",
            "Cargo.lock",
            "scripts/package.py",
            "scripts/ci/classify_paths.py",
            ".github/workflows/cli-packages.yml",
            ".trellis/spec/image.png",
            "unknown/new-file",
        ):
            with self.subTest(path=path):
                self.assertEqual(classify_paths((path,))["scope"], "full")
        self.assertEqual(classify_paths(("gui/src/main.rs", "core-cli/src/main.rs"))["scope"], "full")

    def test_empty_change_set_fails_closed_to_full(self):
        self.assertEqual(classify_paths(())["scope"], "full")

    def test_manual_dispatch_and_first_push_are_full(self):
        expected = {"scope": "full", "docs_changed": False, "run_cli": True}
        self.assertEqual(classify_event("workflow_dispatch", {}), expected)
        self.assertEqual(classify_event("push", {"before": "0" * 40, "after": "1" * 40}), expected)


if __name__ == "__main__":
    unittest.main()
