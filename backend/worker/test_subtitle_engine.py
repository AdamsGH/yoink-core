"""Real yt-dlp track selection and bounded HTTP reads, with synthetic metadata."""
from __future__ import annotations

import copy
import json
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock

import main
import network_guard
from failures import MediaFailure
from subtitle_engine import download_subtitles, validate_options, validate_text

VTT = b"WEBVTT\n\n00:00.000 --> 00:02.000\nManual caption\n"
SRT = b"1\n00:00:00,000 --> 00:00:02,000\nOriginal caption\n"


class SubtitleTests(unittest.TestCase):
    def test_language_is_exact_and_bounded(self):
        for language in ("en", "ru", "zh-Hans", "pt-BR", "eng", "all"):
            validate_options({"language": language, "automatic": True})
        for language in ("", "e", "en.*", "../../x", "en,ru", "en-US-more", "RU", "x" * 17):
            with self.subTest(language=language), self.assertRaises(ValueError):
                validate_options({"language": language, "automatic": False})
        for options in ({"language": "en", "automatic": 1}, {"language": "en", "automatic": False, "extra": True}):
            with self.assertRaises(ValueError):
                validate_options(options)

    def test_protocol_rejects_missing_or_irrelevant_subtitle_options(self):
        base = {"version": 2, "operation_id": "12345678-1234-4234-8234-123456789abc", "attempt": 1,
                "source_url": "https://example.test/video", "task": {"kind": "download", "media_mode": "subtitles"},
                "options": {"subtitles": {"language": "ru", "automatic": True}}}
        def parse(value):
            return main._validate_request(json.dumps(value).encode())
        self.assertEqual(parse(base)["options"]["subtitles"], base["options"]["subtitles"])
        bad = copy.deepcopy(base); bad["options"] = {}
        with self.assertRaises(ValueError): parse(bad)
        bad = copy.deepcopy(base); bad["task"]["media_mode"] = "video"
        with self.assertRaises(ValueError): parse(bad)
        for key, value in (("engine", "gallery"), ("clips", [{"start_seconds": 0, "end_seconds": 1}]), ("gallery_zip", True), ("max_height", 720)):
            bad = copy.deepcopy(base); bad["options"][key] = value
            with self.assertRaises(ValueError): parse(bad)

    def test_text_validation_rejects_html_binary_empty_and_backwards_cues(self):
        validate_text(VTT, "vtt")
        validate_text(SRT, "srt")
        validate_text(b"\xef\xbb\xbf" + VTT, "vtt")
        for data in (b"", b"<html>not subtitles</html>", b"WEBVTT\n", VTT + b"\x00", VTT + b"\xff", VTT.replace(b"00:02.000", b"00:00.000")):
            with self.subTest(data=data), self.assertRaises(MediaFailure):
                validate_text(data, "vtt")

    def run_download(self, metadata, *, language="en", automatic=False, limit=4096, total=8192, items=2):
        import yt_dlp
        def extract(downloader, url, download=True):
            self.assertFalse(download, "standalone subtitles must not download video")
            info = {"id": "fixture", "title": "fixture", "url": "https://example.test/never-fetch.mp4", **copy.deepcopy(metadata)}
            return downloader.process_ie_result(info, download=False)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(yt_dlp.YoutubeDL, "extract_info", extract):
            request = {"source_url": "https://example.test/page", "options": {"subtitles": {"language": language, "automatic": automatic}}}
            artifacts = download_subtitles(request, {"proxy": "", "quiet": True, "no_warnings": True, "cachedir": False, "logger": mock.Mock(), "match_filter": lambda info, **_: "video too large" if info.get("filesize", 0) > limit else None}, Path(directory), limit, total, items, lambda _: None)
            return artifacts, [Path(directory, item["path"]).read_bytes() for item in artifacts]

    def test_manual_tracks_take_priority_and_automatic_is_opt_in(self):
        manual = {"ext": "vtt", "data": VTT.decode()}
        automatic = {"ext": "srt", "data": SRT.decode()}
        meta = {"subtitles": {"en": [manual]}, "automatic_captions": {"en": [automatic]}}
        for enabled in (False, True):
            artifacts, data = self.run_download(meta, automatic=enabled)
            self.assertEqual(data, [VTT])
            self.assertEqual(artifacts[0]["media_type"], "text/vtt")
        meta = {"automatic_captions": {"en": [automatic]}}
        with self.assertRaises(MediaFailure): self.run_download(meta)
        artifacts, data = self.run_download(meta, automatic=True)
        self.assertEqual(data, [SRT])
        self.assertEqual(artifacts[0]["media_type"], "application/x-subrip")

    def test_exact_languages_order_missing_tracks_and_output_limits(self):
        track = {"ext": "vtt", "data": VTT.decode()}
        meta = {"subtitles": {"fr": [track], "ru-orig": [track], "ru": [track], "ru-RU": [track]}}
        artifacts, data = self.run_download(meta, language="ru")
        self.assertEqual([item["path"] for item in artifacts], ["subtitles-ru.vtt", "subtitles-ru-orig.vtt"])
        self.assertEqual(data, [VTT, VTT])
        for kwargs in ({"items": 1}, {"limit": len(VTT) - 1}, {"total": 2 * len(VTT) - 1}):
            with self.subTest(kwargs=kwargs), self.assertRaises(MediaFailure) as error: self.run_download(meta, language="ru", **kwargs)
            self.assertEqual(error.exception.failure_class, "resource_limit")
        for meta in ({}, {"subtitles": {"fr": [track]}}, {"subtitles": {"en": [{"ext": "json3", "data": "{}"}]}}):
            with self.subTest(meta=meta), self.assertRaises(MediaFailure): self.run_download(meta)

    def test_video_size_does_not_apply_to_a_small_subtitle(self):
        meta = {"filesize": 10 * 1024 ** 3, "subtitles": {"en": [{"ext": "vtt", "data": VTT.decode()}]}}
        self.assertEqual(self.run_download(meta, limit=100)[1], [VTT])

    def test_english_keeps_legacy_single_track_and_all_is_not_a_wildcard(self):
        track = {"ext": "vtt", "data": VTT.decode()}
        artifacts, _ = self.run_download({"subtitles": {"en": [track], "en-orig": [track]}})
        self.assertEqual([item["path"] for item in artifacts], ["subtitles-en.vtt"])
        with self.assertRaises(MediaFailure):
            self.run_download({"subtitles": {"en": [track], "ru": [track]}}, language="all")

    def test_guarded_http_and_chunked_size_limits(self):
        body = VTT
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/vtt")
                # No Content-Length: the streaming budget must still stop oversized text.
                self.end_headers(); self.wfile.write(body)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        try:
            meta = {"subtitles": {"en": [{"ext": "vtt", "url": origin + "/track.vtt"}]}}
            with network_guard.guard_context((origin,)):
                self.assertEqual(self.run_download(meta)[1], [VTT])
                with self.assertRaises(MediaFailure) as error: self.run_download(meta, limit=len(VTT) - 1)
                self.assertEqual(error.exception.failure_class, "resource_limit")
            with network_guard.guard_context(()), self.assertRaises(Exception):
                self.run_download(meta)
        finally:
            server.shutdown(); server.server_close(); thread.join()
