from copy import deepcopy
import unittest

from verify_package import validate_selected_media


class SelectedMediaTests(unittest.TestCase):
    def setUp(self):
        self.plan = {
            "video": {"codec": "avc1.64001f", "width": 852, "height": 480},
            "audio": {"codec": "mp4a.40.2", "width": 0, "height": 0,
                      "sample_rate": 48000, "channels": 2},
        }
        self.probe = {"streams": [
            {"media_type": "Video", "codec": "H264", "width": 852, "height": 480},
            {"media_type": "Audio", "codec": "AAC", "width": None, "height": None,
             "sample_rate": 48000, "channels": 2},
        ]}

    def test_audio_dimensions_do_not_constrain_audio(self):
        result = validate_selected_media(self.probe, self.plan)
        self.assertEqual(result["audio"]["actual"]["codec"], "AAC")
        self.assertEqual(result["video"]["actual"]["width"], 852)

    def test_wrong_audio_codec_rate_or_channels_is_rejected(self):
        for field, value in (("codec", "FLAC"), ("sample_rate", 44100), ("channels", 1)):
            with self.subTest(field=field):
                probe = deepcopy(self.probe)
                probe["streams"][1][field] = value
                with self.assertRaises(ValueError):
                    validate_selected_media(probe, self.plan)

    def test_video_dimensions_remain_required_and_exact(self):
        for field in ("width", "height"):
            with self.subTest(field=field):
                probe = deepcopy(self.probe)
                probe["streams"][0][field] += 1
                with self.assertRaises(ValueError):
                    validate_selected_media(probe, self.plan)
                plan = deepcopy(self.plan)
                plan["video"][field] = 0
                with self.assertRaises(ValueError):
                    validate_selected_media(self.probe, plan)


if __name__ == "__main__":
    unittest.main()
