import json
from pathlib import Path
import unittest
from unittest import mock

import verify_package


class SafeErrorReceiptTests(unittest.TestCase):
    def test_http_status_is_preserved_without_unapproved_error_fields(self):
        errors = [
            {
                "code": "Transport",
                "http_status": 503,
                "request_url": "https://private.example/path",
                "response_body": "private-response-marker",
                "headers": {"authorization": "private-header-marker"},
            },
            {"code": "Transport", "http_status": True},
            {"code": "Transport", "http_status": 600},
            {"code": "Transport", "http_status": "503"},
        ]
        process = mock.Mock()
        process.stdout = iter(
            json.dumps({
                "schema_version": 1,
                "type": "error",
                "data": {"error": error, "exit_code": 4},
            }) + "\n"
            for error in errors
        )
        process.stderr = iter(())
        process.poll.return_value = 4
        process.wait.return_value = 4
        process.returncode = 4

        with mock.patch.object(verify_package.subprocess, "Popen", return_value=process):
            with self.assertRaises(RuntimeError) as raised:
                verify_package.interruptible_download(
                    ["bili"], {}, Path("."), Path("state"), timeout=1,
                )

        details = str(raised.exception)
        self.assertIn('"http_status":503', details)
        self.assertNotIn('"http_status":true', details)
        self.assertNotIn('"http_status":600', details)
        self.assertNotIn('"http_status":"503"', details)
        for private_value in ("https://private.example", "private-response-marker", "private-header-marker"):
            self.assertNotIn(private_value, details)


if __name__ == "__main__":
    unittest.main()
