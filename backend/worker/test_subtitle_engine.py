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
ASS = (b"[Script Info]\r\n; realistic ASS header\r\nTitle: Caf\xc3\xa9, sample\r\nScriptType: v4.00+\r\n"
       b"[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, Bold, Italic, Alignment, MarginL, MarginR, MarginV, Encoding\r\n"
       b"Style: Default,Arial,20,&H00FFFFFF,0,0,2,10,10,10,1\r\n"
       b"[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n"
       b"Comment: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,ignored\r\n"
       b"Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,Hello, world! {\\i1}Caf\xc3\xa9{\\i0}\r\n")
SSA = (b"[Script Info]\nScriptType: v4.00\nTitle: SSA fixture\n"
       b"[V4 Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
       b"Style: Default,Arial,20,&H00FFFFFF,0,0,1,1,0,2,10,10,10,1\n"
       b"[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
       b"Dialogue: Marked=0,0:00:00.00,0:00:02.00,Default,,0,0,0,,Bonjour, monde\n")


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
        validate_text(b"WEBVTT\tmetadata\n\n00:00.000 --> 00:02.000\nCaption\twith tab\n", "vtt")
        for data in (b"", b"<html>not subtitles</html>", b"WEBVTT\n", VTT + b"\x00", VTT + b"\xff", VTT.replace(b"00:02.000", b"00:00.000")):
            with self.subTest(data=data), self.assertRaises(MediaFailure):
                validate_text(data, "vtt")

    def test_ass_and_ssa_structure_encoding_and_malformed_inputs(self):
        validate_text(b"\xef\xbb\xbf" + ASS, "ass")
        validate_text(SSA, "ssa")
        invalid = (
            (b"\n" + ASS, "ass"),
            (ASS.replace(b"[Script Info]", b"; leading comment\r\n[Script Info]", 1), "ass"),
            (ASS.replace(b"[Events]", b"[Missing]"), "ass"),
            (ASS.replace(b"[Script Info]", b"[Missing]"), "ass"),
            (ASS.replace(b"Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text", b"Format: Layer, Start, End, Style"), "ass"),
            (ASS.replace(b"Dialogue:", b"Comment:"), "ass"),
            (ASS.replace(b"Effect, Text\r\nComment:", b"Text, Effect\r\nComment:"), "ass"),
            (ASS.replace(b"Text\r\nComment:", b"Text, Text\r\nComment:"), "ass"),
            (ASS.replace(b"[Events]\r\nFormat:", b"[Events]\r\nDialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,early\r\nFormat:"), "ass"),
            (ASS.replace(b"Dialogue: 0,0:00:01.00", b"Dialogue: 0," + b"9" * 5000 + b":00:01.00"), "ass"),
            (ASS.replace(b"0:00:03.50", b"0:00:00.50"), "ass"),
            (ASS + b"\xff", "ass"),
            (ASS + b"\x00", "ass"),
            (SSA.replace(b"[Events]", b"[Missing]"), "ssa"),
            (SSA.replace(b"Text\n", b"Textual\n"), "ssa"),
            (SSA.replace(b"Bonjour, monde", b"  "), "ssa"),
        )
        for data, extension in invalid:
            with self.subTest(extension=extension, data=data[-32:]), self.assertRaises(MediaFailure):
                validate_text(data, extension)

    def test_ass_events_do_not_require_style_sections_and_format_is_bounded(self):
        plain = (b"[Script Info]\nTitle: Plain events\n[Events]\n"
                 b"Format: Start, End, Text\nDialogue: 0:00:00.00,0:00:01.00,Hello, comma\n")
        validate_text(plain, "ass")
        with self.assertRaises(MediaFailure):
            validate_text(plain.replace(b"Format: Start, End, Text", b"Format: Start, End, Text, Actor"), "ass")
        with self.assertRaises(MediaFailure):
            validate_text(plain.replace(b"Dialogue: 0:00:00.00,0:00:01.00,Hello, comma", b"Dialogue: 0:00:00.00,0:00:01.00"), "ass")

    def test_format_preference_and_ass_ssa_mime(self):
        tracks = [
            {"ext": "ssa", "data": SSA.decode()},
            {"ext": "ass", "data": ASS.decode()},
            {"ext": "vtt", "data": VTT.decode()},
            {"ext": "srt", "data": SRT.decode()},
        ]
        artifacts, data = self.run_download({"subtitles": {"en": tracks}})
        self.assertEqual([a["media_type"] for a in artifacts], ["application/x-subrip"])
        self.assertEqual(data, [SRT])
        artifacts, data = self.run_download({"subtitles": {"en": tracks[:3]}})
        self.assertEqual(data, [VTT])
        artifacts, data = self.run_download({"subtitles": {"en": tracks[:2]}})
        self.assertEqual(data, [ASS])
        for track, expected, media_type in ((tracks[1], ASS, "text/x-ssa"), (tracks[0], SSA, "text/x-ssa")):
            artifacts, data = self.run_download({"subtitles": {"en": [track]}})
            self.assertEqual(data, [expected])
            self.assertEqual(artifacts[0]["media_type"], media_type)
        with self.assertRaises(MediaFailure):
            self.run_download({"subtitles": {"en": [{"ext": "json3", "data": "{}"}]}})

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

    def test_invalid_second_track_leaves_no_partial_files(self):
        import yt_dlp
        metadata = {"subtitles": {"ru": [{"ext": "ass", "data": ASS.decode()}],
                                   "ru-orig": [{"ext": "ssa", "data": "invalid"}]}}
        def extract(downloader, url, download=True):
            return downloader.process_ie_result({"id": "fixture", "title": "fixture", **copy.deepcopy(metadata)}, download=False)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(yt_dlp.YoutubeDL, "extract_info", extract):
            with self.assertRaises(MediaFailure):
                download_subtitles({"source_url": "https://example.test/page", "options": {"subtitles": {"language": "ru", "automatic": False}}},
                                   {"quiet": True, "no_warnings": True, "cachedir": False, "logger": mock.Mock()},
                                   Path(directory), 4096, 8192, 2, lambda _: None)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_video_size_does_not_apply_to_a_small_subtitle(self):
        meta = {"filesize": 10 * 1024 ** 3, "subtitles": {"en": [{"ext": "vtt", "data": VTT.decode()}]}}
        self.assertEqual(self.run_download(meta, limit=100)[1], [VTT])

    def test_ass_per_file_and_aggregate_limits(self):
        track = {"ext": "ass", "data": ASS.decode()}
        with self.assertRaises(MediaFailure) as error:
            self.run_download({"subtitles": {"en": [track]}}, limit=len(ASS) - 1)
        self.assertEqual(error.exception.failure_class, "resource_limit")
        meta = {"subtitles": {"ru": [track], "ru-orig": [track]}}
        with self.assertRaises(MediaFailure) as error:
            self.run_download(meta, language="ru", total=2 * len(ASS) - 1)
        self.assertEqual(error.exception.failure_class, "resource_limit")

    def test_automatic_track_selection_uses_exact_requested_languages(self):
        meta = {"automatic_captions": {
            "ru": [{"ext": "ass", "data": ASS.decode()}],
            "ru-RU": [{"ext": "vtt", "data": VTT.decode()}],
            "ru-orig": [{"ext": "ssa", "data": SSA.decode()}],
        }}
        artifacts, data = self.run_download(meta, language="ru", automatic=True)
        self.assertEqual([item["path"] for item in artifacts], ["subtitles-ru.ass", "subtitles-ru-orig.ssa"])
        self.assertEqual(data, [ASS, SSA])

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
