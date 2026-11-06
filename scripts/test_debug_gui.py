import unittest
from pathlib import Path

import debug_gui


class DebugGuiTests(unittest.TestCase):
    def test_private_child_environment_drops_credentials_and_proxy_bypasses(self):
        source = {"SystemRoot": "C:/Windows", "HTTP_PROXY": "http://bad.invalid",
                  "NO_PROXY": "*", "REQUEST_METHOD": "GET", "TOKEN": "do-not-inherit"}
        original = source.copy()
        env = debug_gui.child_environment(Path("C:/repo"), Path("C:/session"), source)
        self.assertEqual(source, original)
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
            self.assertEqual(env[key], debug_gui.DEAD_PROXY)
        self.assertEqual(env["NO_PROXY"], "")
        self.assertNotIn("TOKEN", env)
        self.assertNotIn("REQUEST_METHOD", env)
        self.assertEqual(env["BILI_GUI_ISOLATED_DEBUG_DIR"], str(Path("C:/session/app")))

    def test_live_anonymous_launch_has_no_replay_or_debug_proxy(self):
        env = debug_gui.child_environment(Path("C:/repo"), Path("C:/session"),
            {"SystemRoot": "C:/Windows", "TOKEN": "not-inherited"}, live_anonymous=True)
        self.assertNotIn("BILI_GUI_OFFLINE_REPLAY", env)
        self.assertNotIn("BILI_GUI_ISOLATED_DEBUG_DIR", env)
        self.assertNotIn("HTTP_PROXY", env)
        self.assertNotIn("TOKEN", env)
        self.assertEqual(env["BILI_GUI_ANONYMOUS_LIVE_DIR"], str(Path("C:/session/app")))

    def test_live_launch_preserves_normal_proxy_configuration(self):
        env = debug_gui.child_environment(Path("C:/repo"), Path("C:/session"),
            {"SystemRoot": "C:/Windows", "HTTPS_PROXY": "http://127.0.0.1:8080", "NO_PROXY": "localhost"},
            live_anonymous=True)
        self.assertEqual(env["HTTPS_PROXY"], "http://127.0.0.1:8080")
        self.assertEqual(env["NO_PROXY"], "localhost")
        self.assertNotIn("BILI_GUI_OFFLINE_REPLAY", env)

    def test_scenarios_cover_every_part_and_keep_quality_failures_observable(self):
        responses = debug_gui.fixture_data()["responses"]
        index = {(row["path"], tuple(sorted(row["query"].items()))): row for row in responses}
        self.assertEqual(len(index), len(responses))
        seasons = [row for row in responses if row["query"] == {"season_id": "9001"}]
        self.assertEqual(len(seasons[0]["body"]["result"]["episodes"]), 24)
        for episode in range(9101, 9125):
            for quality in (127, 120, 80, 64):
                key = ("/pgc/player/web/v2/playurl", tuple(sorted({
                    "ep_id": str(episode), "qn": str(quality), "fnval": "12240", "fourk": "1",
                }.items())))
                row = index[key]
                if episode == 9123:
                    self.assertNotEqual(row["body"]["code"], 0)
                else:
                    ids = [stream["id"] for stream in row["body"]["result"]["dash"]["video"]]
                    self.assertNotIn(120, ids)
                    self.assertIn(64, ids)
                    if episode == 9124:
                        self.assertNotIn(80, ids)
        self.assertTrue(any(row["delay_ms"] > 0 for row in responses))

    def test_search_and_category_tabs_have_bounded_exact_responses(self):
        rows = debug_gui.fixture_data()["responses"]
        self.assertLessEqual(len(rows), 512)
        index = {(row["path"], tuple(sorted(row["query"].items()))): row for row in rows}
        self.assertEqual(len(rows), len(index))
        for section in (1, 2, 3, 4, 5, 7):
            query = {"season_type": str(section), "type": "0"}
            self.assertIn(("/pgc/season/index/condition", tuple(sorted(query.items()))), index)
            query.update(style_id="-1", page="1", pagesize="20")
            self.assertTrue(index[("/pgc/season/index/result", tuple(sorted(query.items())))]["body"]["data"]["list"])
        for kind in ("video", "media_bangumi", "media_ft"):
            for page in ("1", "2"):
                query = {"search_type": kind, "keyword": "你好", "page": page}
                self.assertTrue(index[("/x/web-interface/wbi/search/type", tuple(sorted(query.items())))]["body"]["data"]["result"])


if __name__ == "__main__":
    unittest.main()
