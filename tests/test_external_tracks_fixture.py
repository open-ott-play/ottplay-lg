"""The small native-track fixture verifier must reject believable but wrong media.

Pure tests; no encoders, devices, media downloads or private profile storage.
The generator's normal invocation separately probes and decodes its real output.
"""
import copy
import math
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / "fixtures"))
import make_external_tracks_fixture as fixture  # noqa: E402


def probe_document():
    streams = []
    for index, kind, codec, language, title, default, forced in fixture.TEXT_TRACKS:
        streams.append({"index": index, "codec_type": kind, "codec_name": codec,
                        "tags": {"language": language, "title": title},
                        "disposition": {"default": default, "forced": forced},
                        "channels": 1 if kind == "audio" else None,
                        "sample_rate": "48000" if kind == "audio" else "0"})
    return {"streams": streams}


class ExternalTracksFixture(unittest.TestCase):
    def test_interleaved_stream_indexes_have_dense_kind_ordinals(self):
        rows = fixture.validate_inventory(probe_document(), fixture.TEXT_TRACKS)
        self.assertEqual([(r["stream_index"], r["ordinal"]) for r in rows if r["kind"] == "audio"],
                         [(1, 0), (3, 1)])
        self.assertEqual([(r["stream_index"], r["ordinal"]) for r in rows if r["kind"] == "subtitle"],
                         [(2, 0), (4, 1)])

    def test_missing_track_is_not_success(self):
        doc = probe_document()
        doc["streams"].pop()
        with self.assertRaisesRegex(ValueError, "number"):
            fixture.validate_inventory(doc, fixture.TEXT_TRACKS)

    def test_same_codec_audio_swap_is_detected_by_language(self):
        doc = probe_document()
        doc["streams"][1]["tags"], doc["streams"][3]["tags"] = (
            doc["streams"][3]["tags"], doc["streams"][1]["tags"])
        with self.assertRaisesRegex(ValueError, "language"):
            fixture.validate_inventory(doc, fixture.TEXT_TRACKS)

    def test_missing_title_or_forced_or_default_flag_is_not_ignored(self):
        for index, branch, key, value in ((1, "tags", "title", ""),
                                          (1, "disposition", "default", 0),
                                          (4, "disposition", "forced", 0)):
            with self.subTest(key=key):
                doc = probe_document()
                doc["streams"][index][branch][key] = value
                with self.assertRaisesRegex(ValueError, key):
                    fixture.validate_inventory(doc, fixture.TEXT_TRACKS)

    def test_duplicate_container_index_is_not_a_second_track(self):
        doc = probe_document()
        doc["streams"][3]["index"] = 1
        with self.assertRaisesRegex(ValueError, "duplicate"):
            fixture.validate_inventory(doc, fixture.TEXT_TRACKS)

    def test_audio_shape_is_read_not_assumed(self):
        doc = probe_document()
        doc["streams"][1]["channels"] = 2
        with self.assertRaisesRegex(ValueError, "mono"):
            fixture.validate_inventory(doc, fixture.TEXT_TRACKS)

    def test_decoded_tones_are_distinguishable(self):
        for hz in (220, 880):
            samples = [.1 * math.sin(2 * math.pi * hz * i / 8000) for i in range(8000)]
            self.assertAlmostEqual(fixture.measured_tone(samples), hz, delta=2)
        with self.assertRaisesRegex(ValueError, "silent"):
            fixture.measured_tone([0.] * 8000)

    def test_verification_does_not_mutate_probe_input(self):
        doc = probe_document()
        before = copy.deepcopy(doc)
        fixture.validate_inventory(doc, fixture.TEXT_TRACKS)
        self.assertEqual(doc, before)

    def test_pgs_needs_a_decoded_image_and_clear_not_merely_packets(self):
        frames = {"frames": [{"pts_time": "0", "num_rects": 1},
                              {"pts_time": "8", "num_rects": 0}]}
        self.assertEqual(fixture.validate_bitmap_frames(frames), [0, 8])
        frames["frames"][0]["num_rects"] = 0
        with self.assertRaisesRegex(ValueError, "rectangle"):
            fixture.validate_bitmap_frames(frames)

    def test_pgs_clear_cannot_be_missing_or_after_the_video(self):
        for end in (None, 120):
            frames = {"frames": [{"pts_time": "0", "num_rects": 1}]}
            if end is not None:
                frames["frames"].append({"pts_time": str(end), "num_rects": 0})
            with self.assertRaisesRegex(ValueError, "clear"):
                fixture.validate_bitmap_frames(frames)


if __name__ == "__main__":
    unittest.main()
