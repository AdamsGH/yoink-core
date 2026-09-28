"""Container-only tests for the media worker; no external network is used."""
from __future__ import annotations

import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest import mock

import main
import media_engine
import network_guard


WORKER_DIR = Path(__file__).resolve().parent


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.previous = network_guard._ALLOWED_ORIGINS

    def tearDown(self):
        network_guard._ALLOWED_ORIGINS = self.previous
        socket.getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

    def test_filters_private_mixed_dns_and_all_sensitive_addresses(self):
        network_guard.install_guard(())
        public = (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("8.8.8.8", 80))
        unsafe = [
            "127.0.0.1", "10.0.0.1", "169.254.169.254", "224.0.0.1",
            "192.0.2.1", "::1", "fe80::1", "::ffff:127.0.0.1",
        ]
        for address in unsafe:
            family = socket.AF_INET6 if ":" in address else socket.AF_INET
            entry = (family, socket.SOCK_STREAM, 6, "", (address, 80, 0, 0) if family == socket.AF_INET6 else (address, 80))
            with self.subTest(address=address), mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=[public, entry]):
                with self.assertRaises(OSError):
                    network_guard.guarded_getaddrinfo("mixed.example", 80)
        with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=[public]):
            self.assertEqual(network_guard.guarded_getaddrinfo("public.example", 80), [public])

    def test_numeric_private_and_ipv4_mapped_are_denied_without_exact_allowlist(self):
        network_guard.install_guard(())
        for url in ("http://127.0.0.1/video", "http://[::ffff:127.0.0.1]/video", "http://169.254.169.254/latest"):
            with self.subTest(url=url), self.assertRaises(OSError):
                network_guard.validate_source_url(url, ())

    def test_allowlist_is_exact_hostname_and_port_and_applies_redirect_resolution(self):
        network_guard.install_guard(("http://fixture.test:8123/",))
        private = (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 8123))
        with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=[private]):
            network_guard.validate_source_url("http://fixture.test:8123/media", ("http://fixture.test:8123",))
            with self.assertRaises(OSError):
                network_guard.guarded_getaddrinfo("fixture.test", 8124)
            with self.assertRaises(OSError):
                network_guard.guarded_getaddrinfo("redirect.test", 8123)


class ProtocolAndSettingsTests(unittest.TestCase):
    def test_download_error_classification_is_specific(self):
        class DownloadError(Exception):
            pass

        for status in (408, 500, 502, 503, 504):
            with self.subTest(status=status):
                self.assertEqual(main._classify_error(DownloadError(f"ERROR: HTTP Error {status}: unavailable")), "source_unavailable")
        self.assertEqual(main._classify_error(DownloadError("ERROR: HTTP Error 429: Too Many Requests")), "rate_limited")
        for message in (
            "HTTP Error 400: Bad Request", "HTTP Error 401: Unauthorized",
            "HTTP Error 403: Forbidden", "HTTP Error 404: Not Found",
            "media has 503 frames", "codec has 408 frames",
        ):
            with self.subTest(message=message):
                self.assertEqual(main._classify_error(DownloadError(message)), "invalid_media")
        self.assertEqual(main._classify_error(DownloadError("Connection reset by peer")), "source_unavailable")

    def test_ffprobe_output_limit_and_timeout_kill_and_reap_child(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_file = root / "child.pid"
            for name, body, timeout in (
                ("large", "echo $$ > $PID_FILE; while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done", 5),
                ("hang", "echo $$ > $PID_FILE; exec sleep 30", 1),
            ):
                executable = root / name
                executable.write_text("#!/bin/sh\n" + body + "\n")
                executable.chmod(0o755)
                with mock.patch.dict(os.environ, {"PID_FILE": str(pid_file)}):
                    with self.assertRaises(media_engine.MediaFailure):
                        media_engine._probe(root / "fixture.mp4", str(executable), timeout)
                pid = int(pid_file.read_text())
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)
                pid_file.unlink()

    def test_ffprobe_accepts_actual_video(self):
        with tempfile.TemporaryDirectory() as directory:
            video = Path(directory) / "fixture.mp4"
            subprocess.run([
                "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=64x64:d=0.25", "-c:v", "mpeg4", str(video),
            ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            info, _ = media_engine._probe(video, "ffprobe", 10)
            self.assertTrue(any(item.get("codec_type") == "video" for item in info["streams"]))

    def test_request_validation_and_oversize(self):
        good = {
            "version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce",
            "attempt": 1, "source_url": "https://example.test/video",
            "task": {"kind": "download", "media_mode": "video"},
        }
        validated = main._validate_request(json.dumps(good).encode())
        self.assertEqual({key: validated[key] for key in good}, good)
        self.assertEqual(validated["options"]["video_codec"], "h264")
        invalids = []
        invalids.append({**good, "version": 1})
        invalids.append({**good, "unexpected": True})
        invalids.append({**good, "source_url": "https://user:pw@example.test/video"})
        invalids.append({**good, "task": {"kind": "download", "media_mode": "audio", "unknown": 1}})
        for request in invalids:
            with self.subTest(request=request), self.assertRaises(ValueError):
                main._validate_request(json.dumps(request).encode())
        with self.assertRaises(ValueError):
            main._validate_request(b" " * (main.MAX_FRAME_BYTES + 1))
        with self.assertRaises(ValueError):
            main._validate_request(b"not-json")

    def test_playlist_protocol_preserves_domain_order_and_signed_bounds(self):
        base = {
            "version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce",
            "attempt": 1, "source_url": "https://example.test/playlist",
            "task": {"kind": "download", "media_mode": "video"},
        }
        cases = (
            (4, 2, [4, 3, 2]),
            (-3, -1, [-3, -2, -1]),
            (-1, -3, [-1, -2, -3]),
            (-3, None, [-3, -2, -1]),
            (7, None, [7]),
            (1, 20, list(range(1, 21))),
            (1, 100, list(range(1, 101))),
            (-2147483648, -2147483648, [-2147483648]),
            (2147483647, 2147483647, [2147483647]),
        )
        for start, end, expected in cases:
            with self.subTest(start=start, end=end):
                raw = {**base, "options": {"playlist": {"start": start, "end": end}}}
                validated = main._validate_request(json.dumps(raw).encode())
                self.assertEqual(media_engine.playlist_indices(validated["options"]["playlist"]), expected)
        for start, end in (
            (0, None), (1, 0), (1, -1), (-1, 1), (1, 101), (-101, None),
            (-2147483649, -2147483649), (2147483648, 2147483648), (True, 1), (1, False),
        ):
            with self.subTest(start=start, end=end), self.assertRaises(ValueError):
                main._validate_request(json.dumps({**base, "options": {
                    "playlist": {"start": start, "end": end},
                }}).encode())

    def test_task_audio_and_extract_content_are_classified_not_corrupt(self):
        for task in ({"kind": "download", "media_mode": "audio"}, {"kind": "extract_content"}):
            request = {
                "version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce",
                "attempt": 1, "source_url": "https://example.test/video", "task": task,
            }
            validated = main._validate_request(json.dumps(request).encode())
            self.assertEqual({key: validated[key] for key in request}, request)

    def test_engine_resource_failure_preserves_class_without_diagnostics(self):
        request = {"version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce", "attempt": 1, "source_url": "https://8.8.8.8/private-token", "task": {"kind": "download", "media_mode": "audio"}}
        events = []
        with mock.patch.object(main, "download_media", side_effect=media_engine.MediaFailure("resource_limit")), mock.patch.object(main, "_emit", side_effect=events.append):
            main._download(request, main.Settings.from_env({}))
        self.assertEqual([item["event"] for item in events], [{"type": "failure", "class": "resource_limit"}])
        self.assertNotIn("private-token", json.dumps(events))

    def test_settings_ranges_and_origin_shape(self):
        self.assertEqual(main.Settings.from_env({}).max_bytes, 50 * 1024 * 1024)
        for key, value in (
            ("YOINK_WORKER_MAX_BYTES", "0"), ("YOINK_WORKER_MAX_BYTES", str(2 * 1024 ** 3 + 1)),
            ("YOINK_WORKER_SOCKET_TIMEOUT_SECS", "301"),
            ("YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS", '["http://host/path"]'),
            ("YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS", '"not-array"'),
        ):
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                main.Settings.from_env({key: value})


class IntegrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.video = Path(cls.tmp.name) / "tiny.mp4"
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
            "color=c=black:s=64x64:d=0.25", "-f", "lavfi", "-i",
            "anullsrc=r=44100:cl=stereo", "-shortest", "-c:v", "mpeg4", "-c:a",
            "aac", "-movflags", "+faststart", str(cls.video),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        video = self.video.read_bytes()

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/status503":
                    self.send_error(503, "temporary fixture failure")
                    return
                if self.path != "/tiny.mp4":
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", "video/mp4")
                self.send_header("Content-Length", str(len(video)))
                self.end_headers()
                self.wfile.write(video)

            def log_message(self, _format, *args):
                return

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def invoke(self, allowed: bool, max_bytes: int = 50 * 1024 * 1024, path: str = "/tiny.mp4"):
        request = {
            "version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce",
            "attempt": 2, "source_url": self.origin + path,
            "task": {"kind": "download", "media_mode": "video"},
        }
        env = os.environ.copy()
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            env.pop(key, None)
        env.update({
            "YOINK_WORKER_MAX_BYTES": str(max_bytes),
            "YOINK_WORKER_SOCKET_TIMEOUT_SECS": "10",
            "YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS": json.dumps([self.origin] if allowed else []),
        })
        with tempfile.TemporaryDirectory() as cwd:
            result = subprocess.run(
                [sys.executable, "-I", "-B", str(WORKER_DIR / "main.py")],
                input=json.dumps(request).encode() + b"\n", cwd=cwd, env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30,
            )
            return result, {entry.name for entry in Path(cwd).iterdir()}

    def test_real_ytdlp_download_and_redacted_protocol(self):
        result, output_names = self.invoke(True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stderr, b"")
        events = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertTrue(events)
        self.assertTrue(all(event["operation_id"] == "4f15ab1d-8f81-4886-999d-44c26db8b9ce" and event["attempt"] == 2 for event in events))
        self.assertTrue(any(event["event"]["type"] == "progress" for event in events))
        terminal = [event for event in events if event["event"]["type"] != "progress"]
        self.assertEqual(len(terminal), 1)
        complete = terminal[0]["event"]
        self.assertEqual(complete["type"], "completed")
        artifact = complete["artifacts"][0]
        self.assertEqual(artifact["path"], "media-001.mp4")
        self.assertGreater(artifact["size_bytes"], 0)
        self.assertEqual(artifact["media_type"], "video/mp4")
        self.assertEqual(output_names, {"media-001.mp4"})
        self.assertNotIn(self.origin, result.stdout.decode())

    def test_real_ytdlp_http_503_is_source_unavailable_and_redacted(self):
        result, output_names = self.invoke(True, path="/status503")
        self.assertEqual(result.returncode, 0)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(events[-1]["event"], {"type": "failure", "class": "source_unavailable"})
        self.assertEqual(output_names, set())
        self.assertNotIn(self.origin, result.stdout.decode())
        self.assertEqual(result.stderr, b"")

    def test_private_origin_requires_exact_allowlist(self):
        result, _ = self.invoke(False)
        self.assertEqual(result.returncode, 0)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(events[-1]["event"], {"type": "failure", "class": "source_unavailable"})
        self.assertNotIn(self.origin, result.stdout.decode())

    def test_oversized_media_is_resource_limit(self):
        result, _ = self.invoke(True, max_bytes=100)
        self.assertEqual(result.returncode, 0)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(events[-1]["event"], {"type": "failure", "class": "resource_limit"})
        self.assertNotIn(self.origin, result.stdout.decode())


if __name__ == "__main__":
    unittest.main()
