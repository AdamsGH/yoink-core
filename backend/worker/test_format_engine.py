"""Metadata-only format reporting and hostile metadata bounds."""
from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import main
import yt_dlp
from failures import MediaFailure
from format_engine import HEADER, download_format_report


def request(options=None):
    return {"source_url": "https://example.test/watch", "task": {"kind": "download", "media_mode": "formats"},
            "options": {"video_codec": "h264", "audio_codec": "best", "container": "mp4", "engine": "auto",
                         "max_height": None, "playlist": None, "clips": [], "gallery_zip": False,
                         **(options or {})}}


class FormatReportTests(unittest.TestCase):
    def run_report(self, metadata, *, maximum=50 * 1024):
        with tempfile.TemporaryDirectory() as directory:
            calls = []
            original_init = yt_dlp.YoutubeDL.__init__
            def init(downloader, opts=None):
                calls.append(opts.copy())
                original_init(downloader, opts)
            def extract(_downloader, url, *, download):
                calls.append((url, download))
                return metadata
            with mock.patch.object(yt_dlp.YoutubeDL, "__init__", autospec=True, side_effect=init), \
                    mock.patch.object(yt_dlp.YoutubeDL, "extract_info", autospec=True, side_effect=extract), \
                    mock.patch.object(yt_dlp.YoutubeDL, "download", side_effect=AssertionError("media download attempted")):
                result = download_format_report(request(), {"quiet": True}, Path(directory), maximum, maximum, 20, lambda _: None)
            return result, (Path(directory) / "formats.txt").read_bytes(), calls

    def test_formats_are_selected_and_encoded_with_exact_safe_grammar(self):
        meta = {"formats": [
            {"format_id": "137", "ext": "mp4", "width": 1920, "height": 1080, "fps": 29.97,
             "vcodec": "avc1.640028", "acodec": "none", "protocol": "https"},
            {"format_id": "x\tsecret", "ext": "M4A", "width": -1, "height": 16400, "fps": 1000.1234,
             "vcodec": "malicious codec", "acodec": "mp4a.40.2", "protocol": "http_dash_segments"},
            {"format_id": "18", "ext": "webm", "vcodec": "none", "acodec": "opus", "protocol": "m3u8_native"},
            {"format_id": "666", "ext": "mp4", "vcodec": "h264", "acodec": "aac", "has_drm": True},
        ], "title": "signed-title-secret", "url": "https://secret.test/?token=signed-secret",
            "http_headers": {"Cookie": "cookie-secret"}}
        artifacts, data, calls = self.run_report(meta)
        self.assertEqual(artifacts, [{"path": "formats.txt", "size_bytes": len(data), "media_type": "text/plain"}])
        self.assertEqual(data.decode(), HEADER + "137\tmp4\t1920\t1080\t29.97\th264\tnone\thttps\n"
                         "-\tm4a\t-\t-\t-\tunknown\taac\tdash\n18\twebm\t-\t-\t-\tnone\topus\thls\n")
        for secret in (b"signed-title-secret", b"signed-secret", b"cookie-secret", b"malicious codec"):
            self.assertNotIn(secret, data)
        self.assertTrue(calls[0]["skip_download"])
        self.assertTrue(calls[0]["noplaylist"])
        self.assertEqual(calls[0]["postprocessors"], [])
        self.assertIsNone(calls[0]["format"])
        self.assertEqual(calls[-1], ("https://example.test/watch", False))
        self.assertEqual(len(data.splitlines()), 5)

    def test_direct_media_with_unknown_codecs_is_not_discarded_as_a_storyboard(self):
        _, data, _ = self.run_report({"formats": [
            {"format_id": "mp4", "ext": "mp4", "protocol": "http"},
            {"format_id": "audio", "ext": "mp3", "vcodec": "none", "protocol": "https"},
            {"format_id": "storyboard", "ext": "mp4", "vcodec": "none", "acodec": "none"},
        ]})
        self.assertEqual(data.decode(), HEADER + "-\tmp4\t-\t-\t-\tunknown\tunknown\thttp\n"
                         "-\tmp3\t-\t-\t-\tnone\tunknown\thttps\n")

    def test_rejects_playlist_live_missing_and_malformed_metadata(self):
        for data in ({"_type": "playlist", "entries": []}, {"is_live": True, "formats": []},
                     {"formats": []}, {"formats": [None]}):
            with self.subTest(data=data), self.assertRaises(MediaFailure):
                self.run_report(data)

    def test_bounds_format_count_and_report_size_without_truncation(self):
        row = {"format_id": "1", "ext": "mp4", "vcodec": "h264", "acodec": "none"}
        with self.assertRaises(MediaFailure):
            self.run_report({"formats": [row] * 101})
        with self.assertRaises(MediaFailure):
            self.run_report({"formats": [row] * 2}, maximum=len(HEADER.encode()) + 5)

    def test_protocol_requires_default_unmodified_options(self):
        base = {"version": 2, "operation_id": "12345678-1234-4234-8234-123456789abc", "attempt": 1,
                "source_url": "https://example.test/watch", "task": {"kind": "download", "media_mode": "formats"},
                "options": {}}
        parsed = main._validate_request(json.dumps(base).encode())
        self.assertEqual(parsed["options"]["video_codec"], "h264")
        for key, value in (("send_as_file", True), ("max_height", 720),
                           ("playlist", {"start": 1, "end": 2}), ("clips", [{"start_seconds": 0, "end_seconds": 1}]),
                           ("gallery_zip", True), ("engine", "gallery"),
                           ("subtitles", {"language": "en", "automatic": False})):
            bad = json.loads(json.dumps(base))
            bad["options"][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                main._validate_request(json.dumps(bad).encode())


if __name__ == "__main__":
    unittest.main()
