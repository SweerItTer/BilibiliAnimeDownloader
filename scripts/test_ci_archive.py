import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.request
import urllib.response

SPEC = importlib.util.spec_from_file_location("archive_assets", Path(__file__).parent / "ci/archive_assets.py")
archive_assets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(archive_assets)


class ArchiveApi:
    def __init__(self):
        self.payload = b"opaque Actions artifact ZIP"
        self.deleted = []
        self.run = {"id": 99, "repository": {"id": 7, "html_url": "https://github.com/owner/repo"},
                    "status": "completed", "conclusion": "success", "workflow_id": 11, "run_attempt": 1,
                    "head_sha": "a" * 40, "created_at": "2026-10-29T00:00:00Z",
                    "html_url": "https://github.com/owner/repo/actions/runs/99"}

    def json(self, path, method="GET"):
        if method == "DELETE":
            self.deleted.append(path)
            return None
        return {"": {"id": 7}, "actions/runs/99": self.run,
                "actions/workflows/cli-packages.yml": {"id": 11, "path": ".github/workflows/cli-packages.yml"},
                "actions/workflows/release.yml": {"id": 12, "path": ".github/workflows/release.yml"}}[path]

    def artifacts(self, run_id):
        return [{"id": 10, "name": "linux-cli"}, {"id": 11, "name": "windows-cli"},
                {"id": 13, "name": "windows-gui"},
                {"id": 12, "name": "internal-sdk", "expired": False,
                 "digest": "sha256:" + hashlib.sha256(self.payload).hexdigest()}]

    def download(self, artifact_id, output):
        output.write_bytes(self.payload)


class ArchiveSafetyTests(unittest.TestCase):
    def registry(self, api, failure=None):
        registry = archive_assets.Registry("registry.test/owner/repo", "", "")
        digest = "sha256:" + "a" * 64

        def command(action, *args, cwd=None):
            if action == "push":
                return json.dumps({"digest": digest})
            if action == "resolve":
                return "sha256:" + "b" * 64 if failure == "digest" else digest
            if action == "pull":
                output = Path(args[args.index("--output") + 1]) / "artifact-12.zip"
                output.write_bytes(b"damaged registry payload" if failure == "bytes" else api.payload)
                return ""
            raise AssertionError(action)

        registry.command = command
        return registry

    def test_only_verified_internal_artifact_is_removed(self):
        api = ArchiveApi()
        archived = archive_assets.archive_run(api, self.registry(api), 99)
        self.assertEqual(api.deleted, ["actions/artifacts/12"])
        self.assertEqual([item["artifact_id"] for item in archived], [12])
        self.assertEqual(archived[0]["sha256"], hashlib.sha256(api.payload).hexdigest())

    def test_manifest_mismatch_or_corrupt_readback_never_deletes(self):
        for failure in ("digest", "bytes"):
            with self.subTest(failure=failure):
                api = ArchiveApi()
                with self.assertRaises(SystemExit):
                    archive_assets.archive_run(api, self.registry(api, failure), 99)
                self.assertEqual(api.deleted, [])

    def test_unrelated_incomplete_and_failed_runs_are_not_archived(self):
        for field, value in (("workflow_id", 100), ("repository", {"id": 8}),
                             ("status", "in_progress"), ("conclusion", "failure")):
            with self.subTest(field=field):
                api = ArchiveApi()
                api.run[field] = value
                with self.assertRaises(SystemExit):
                    archive_assets.archive_run(api, self.registry(api), 99)
                self.assertEqual(api.deleted, [])

    def test_unknown_product_layout_prevents_legacy_cleanup(self):
        api = ArchiveApi()
        internal = api.artifacts(99)[-1]
        api.artifacts = lambda run_id: [internal]
        with self.assertRaises(SystemExit):
            archive_assets.archive_run(api, self.registry(api), 99)
        self.assertEqual(api.deleted, [])

    def test_storage_redirect_does_not_receive_github_credentials(self):
        requests = []
        payload = b"signed storage response"

        class Transport(urllib.request.HTTPSHandler):
            handler_order = 400

            def https_open(self, request):
                requests.append(request)
                if request.host == "api.github.com":
                    response = urllib.response.addinfourl(io.BytesIO(),
                        {"Location": "https://storage.example/artifact?signature=test-only"}, request.full_url, 302)
                    response.msg = "Found"
                else:
                    response = urllib.response.addinfourl(io.BytesIO(payload), {}, request.full_url, 200)
                    response.msg = "OK"
                return response

        api = archive_assets.GitHub("owner/repo", "synthetic-test-token")
        api.opener.add_handler(Transport())
        with tempfile.TemporaryDirectory() as temporary, patch.object(archive_assets.urllib.request, "urlopen", api.opener.open):
            output = Path(temporary) / "artifact.zip"
            api.download(12, output)
            self.assertEqual(output.read_bytes(), payload)
        self.assertEqual(requests[0].get_header("Authorization"), "Bearer synthetic-test-token")
        self.assertIsNone(requests[1].get_header("Authorization"))


if __name__ == "__main__":
    unittest.main()
