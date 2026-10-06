"""Real pinned extraction through local HTTP metadata; no host Python."""
from __future__ import annotations
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import os

import main
from failures import MediaFailure
from search_engine import normalize_entries, validate_query, search, MAX_RESPONSE_BYTES
from search_fixture import local_extraction


def request(query="cats"):
    return {"version": 2, "operation_id": "12345678-1234-1234-1234-123456789012", "attempt": 1,
            "task": {"kind": "youtube_search", "query": query}, "network": None}


class SearchTests(unittest.TestCase):
    def extract(self, query):
        with local_extraction() as (origin, calls), tempfile.TemporaryDirectory() as directory:
            previous = os.getcwd()
            os.chdir(directory)
            try:
                settings = main.Settings(1024, 1024, 1, 2, (origin,), ())
                output = io.StringIO()
                with mock.patch("sys.stdout", output):
                    main._search(main._validate_request(json.dumps(request(query)).encode()), settings)
                self.assertEqual(list(Path(directory).iterdir()), [], "metadata lookup created artifacts")
                return json.loads(output.getvalue()), calls
            finally:
                os.chdir(previous)

    def test_actual_pinned_search_extraction_zero_eight_duplicate_oversized_unicode(self):
        for query, expected in (("cats", 8), ("zero", 0), ("duplicates", 1), ("oversized", 8), ("strings", 1)):
            with self.subTest(query=query):
                event, calls = self.extract(query)
                self.assertEqual(event["event"]["type"], "search_completed", event)
                entries = event["event"]["entries"]
                self.assertEqual(len(entries), expected)
                self.assertEqual(len({entry["url"] for entry in entries}), expected)
                self.assertTrue(all(call["path"].startswith("/youtubei/v1/search") and call["cookie"] is None for call in calls))
                self.assertGreater(len(calls), 0)
                if entries:
                    self.assertEqual(entries[0]["url"], "https://www.youtube.com/watch?v=fixture0000")
                    self.assertEqual(entries[0]["duration_seconds"], 62)
                    self.assertEqual(entries[0]["view_count"], 1234)
                    self.assertLessEqual(len(entries[0]["title"]), 120)
                    self.assertNotIn("\u202e", entries[0]["title"])

    def test_malformed_pinned_metadata_fails_without_artifacts_or_source_diagnostics(self):
        event, _ = self.extract("malformed")
        self.assertEqual(event["event"], {"type": "failure", "class": "invalid_media"})

    def test_network_denial_is_failure_not_empty_success_and_secret_free(self):
        event, _ = self.extract("network denial")
        self.assertEqual(event["event"]["type"], "failure")
        self.assertNotIn("secret", json.dumps(event))
        self.assertNotIn("127.0.0.2", json.dumps(event))

    def test_metadata_http_budget_and_configured_proxy_failure_forbid_direct_fallback(self):
        event, _ = self.extract("response overflow")
        self.assertEqual(event["event"], {"type": "failure", "class": "resource_limit"})
        with local_extraction() as (origin, calls):
            raw = {**request(), "network": {"proxy_url": "http://user:secret@127.0.0.1:1"}}
            settings = main.Settings(1024, 1024, 1, 1, (origin,), ("http://127.0.0.1:1",))
            output = io.StringIO()
            with mock.patch("sys.stdout", output):
                main._search(main._validate_request(json.dumps(raw).encode()), settings)
            event = json.loads(output.getvalue())
            self.assertEqual(event["event"]["type"], "failure")
            self.assertEqual(calls, [], "proxy failure silently fell back to direct egress")
            self.assertNotIn("secret", output.getvalue())

    def test_typed_parser_rejects_incomplete_source_cookie_and_query_shapes(self):
        for change in ({"source_url": "ytsearch8:secret"}, {"options": {}}, {"network": {"cookie_jar": "secret"}},
                       {"task": {"kind": "youtube_search"}}, {"task": {"kind": "youtube_search", "query": "x"}},
                       {"task": {"kind": "youtube_search", "query": "x" * 257}}, {"version": 3}):
            with self.subTest(change=change), self.assertRaises(ValueError) as error:
                main._validate_request(json.dumps({**request(), **change}).encode())
            self.assertNotIn("secret", str(error.exception))
        for query in ("Кот", "http2 explained", "http proxy setup", "x" * 256):
            validate_query(query)
        for query in ("", "x", "x" * 257, "cats\n", "cats\u202e"):
            with self.assertRaises(ValueError):
                validate_query(query)

    def test_search_selection_watch_fixture_uses_real_pinned_video_metadata_parser(self):
        import yt_dlp
        from network_guard import guard_context
        class SilentLogger:
            def debug(self, *_args): pass
            def warning(self, *_args): pass
            def error(self, *_args): pass
        with local_extraction(video=True) as (origin, _calls), guard_context((origin,), ()):
            with yt_dlp.YoutubeDL({"quiet": True, "no_warnings": True, "logger": SilentLogger(), "skip_download": True, "cachedir": False, "ignoreconfig": True, "proxy": "", "plugin_dirs": [], "retries": 0, "extractor_retries": 0}) as downloader:
                info = downloader.extract_info("https://www.youtube.com/watch?v=fixture0000", download=False)
            self.assertEqual(info["id"], "fixture0000")
            self.assertEqual(info["title"], "Ordinary inline fixture")
            self.assertTrue(any(fmt.get("url") == origin + "/media.mp4" for fmt in info["formats"]))

    def test_bad_provider_urls_metadata_and_bounds_are_never_forwarded(self):
        valid = {"id": "fixture0000", "url": "https://www.youtube.com/watch?v=fixture0000", "title": "x" * 1000,
                 "channel": "y" * 1000, "duration": float("nan"), "view_count": True}
        entries = [None, {}, {**valid, "url": "https://evil.example/watch?v=fixture0000"},
                   {**valid, "url": "https://user:secret@www.youtube.com/watch?v=fixture0000"},
                   {**valid, "url": valid["url"] + "&token=secret"}, valid, valid]
        result = normalize_entries(entries)
        self.assertEqual(len(result), 1)
        self.assertEqual(len(result[0]["title"]), 120)
        self.assertEqual(len(result[0]["channel"]), 100)
        self.assertIsNone(result[0]["duration_seconds"])
        self.assertIsNone(result[0]["view_count"])
        with self.assertRaises(MediaFailure):
            normalize_entries([valid] * 65)
