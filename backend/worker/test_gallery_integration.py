"""Exercise pinned gallery-dl networking and file writes against loopback only."""
from __future__ import annotations

import json
import logging
import os
import re
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock

from gallery_dl import extractor
from gallery_dl.extractor.common import Extractor, Message

import main


def png_fixture() -> bytes:
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress((b"\0" + b"\xff\0\0" * 2) * 2)) + chunk(b"IEND", b"")


class GalleryIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.payload = png_fixture()
        self.fail_last = False
        self.hits = []
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.hits.append(self.path)
                if owner.fail_last and self.path == "/image-3.png":
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(owner.payload)))
                self.end_headers()
                self.wfile.write(owner.payload)
            def log_message(self, *_args):
                pass
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5)

    def invoke(self, count=3, max_items=20, max_total=4096, gallery_zip=False, max_bytes=4096):
        origin = self.origin
        class FixtureExtractor(Extractor):
            category = "yoinkfixture"
            subcategory = "gallery"
            directory_fmt = ()
            filename_fmt = "{num:>04}.{extension}"
            def items(self):
                yield Message.Directory, None, {}
                for index in range(1, count + 1):
                    yield Message.Url, f"{origin}/image-{index}.png", {"num": index, "extension": "png"}
        request = main._validate_request(json.dumps({
            "version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce", "attempt": 1,
            "source_url": origin + "/gallery", "task": {"kind": "download", "media_mode": "image"},
            "options": {"engine": "gallery", "gallery_zip": gallery_zip},
        }).encode())
        settings = main.Settings.from_env({
            "YOINK_WORKER_MAX_BYTES": str(max_bytes), "YOINK_WORKER_TOTAL_MAX_BYTES": str(max_total),
            "YOINK_WORKER_MAX_ITEMS": str(max_items), "YOINK_WORKER_SOCKET_TIMEOUT_SECS": "3",
            "YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS": json.dumps([origin]),
        })
        events = []
        with tempfile.TemporaryDirectory() as directory:
            old = Path.cwd()
            try:
                os.chdir(directory)
                previous_disable = logging.root.manager.disable
                logging.disable(logging.NOTSET)
                try:
                    with self.assertLogs("yoinkfixture", level="DEBUG") as diagnostics, mock.patch.object(extractor, "find", side_effect=lambda url: FixtureExtractor(re.fullmatch(".*", url))), mock.patch.object(main, "_emit", side_effect=events.append):
                        main._download(request, settings)
                    self.diagnostics = diagnostics.output
                finally:
                    logging.disable(previous_disable)
                files = {str(path.relative_to(directory)): path.read_bytes() for path in Path(directory).rglob("*") if path.is_file()}
            finally:
                os.chdir(old)
        return [event["event"] for event in events], files

    def test_real_gallery_download_and_png_probe(self):
        events, files = self.invoke()
        self.assertEqual(events[-1]["type"], "completed", (events, self.diagnostics))
        artifacts = events[-1]["artifacts"]
        self.assertEqual(len(artifacts), 3)
        self.assertEqual([item["path"] for item in artifacts], ["0001.png", "0002.png", "0003.png"])
        self.assertTrue(all(item["media_type"] == "image/png" for item in artifacts))
        self.assertEqual(set(files), {item["path"] for item in artifacts})
        self.assertTrue(all(data == self.payload for data in files.values()))

    def test_aggregate_budget_rejects_before_overflow_write(self):
        maximum = len(self.payload) * 2 + len(self.payload) // 2
        events, files = self.invoke(max_total=maximum)
        self.assertEqual(events[-1], {"type": "failure", "class": "resource_limit"})
        self.assertLessEqual(sum(map(len, files.values())), maximum)
        self.assertEqual(len([data for data in files.values() if data == self.payload]), 2)

    def test_gallery_subprocess_stdout_is_only_protocol_frames(self):
        script = '''
import re
from urllib.parse import urlsplit
from gallery_dl import extractor
from gallery_dl.extractor.common import Extractor, Message
import main
class FixtureExtractor(Extractor):
    category = "yoinkfixture"
    subcategory = "gallery"
    directory_fmt = ()
    filename_fmt = "{num:>04}.{extension}"
    def items(self):
        source = urlsplit(self.url)
        yield Message.Directory, None, {}
        for index in range(1, 4):
            yield Message.Url, f"{source.scheme}://{source.netloc}/image-{index}.png", {"num": index, "extension": "png"}
extractor.find = lambda url: FixtureExtractor(re.fullmatch(".*", url))
main.main()
'''
        request = {"version": 2, "operation_id": "4f15ab1d-8f81-4886-999d-44c26db8b9ce", "attempt": 1,
                   "source_url": self.origin + "/gallery", "task": {"kind": "download", "media_mode": "image"},
                   "options": {"engine": "gallery"}}
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, "-B", "-c", script], input=json.dumps(request).encode() + b"\n",
                                    cwd=directory, capture_output=True, timeout=15,
                                    env={"PATH": os.environ["PATH"], "PYTHONPATH": str(Path(main.__file__).parent),
                                         "YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS": json.dumps([self.origin])})
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            lines = result.stdout.splitlines()
            self.assertTrue(lines)
            self.assertTrue(all(line.startswith(b"{") for line in lines), result.stdout)
            events = [json.loads(line) for line in lines]
            self.assertEqual(events[-1]["event"]["type"], "completed", events)
            self.assertEqual(len(events[-1]["event"]["artifacts"]), 3)
            self.assertNotIn(b"Traceback", result.stderr)

    def test_per_file_limit_is_not_a_silent_gallery_skip(self):
        maximum = len(self.payload) - 1
        events, files = self.invoke(max_bytes=maximum)
        self.assertEqual(events[-1], {"type": "failure", "class": "resource_limit"})
        self.assertTrue(all(len(data) <= maximum for data in files.values()))

    def test_item_budget_stops_before_third_image_write(self):
        events, files = self.invoke(max_items=2)
        self.assertEqual(events[-1], {"type": "failure", "class": "resource_limit"})
        self.assertEqual(len([data for data in files.values() if data == self.payload]), 2)

    def test_failed_gallery_item_never_returns_partial_success(self):
        self.fail_last = True
        events, _ = self.invoke()
        self.assertEqual(events[-1]["type"], "failure")
        self.assertFalse(any(event["type"] == "completed" for event in events))

    def test_large_gallery_returns_nine_images_and_archive(self):
        events, files = self.invoke(count=12, gallery_zip=True)
        self.assertEqual(events[-1]["type"], "completed", (events, self.diagnostics))
        artifacts = events[-1]["artifacts"]
        self.assertEqual(len(artifacts), 10)
        self.assertEqual(artifacts[-1]["media_type"], "application/zip")
        import io
        import zipfile
        with zipfile.ZipFile(io.BytesIO(files[artifacts[-1]["path"]])) as archive:
            self.assertEqual(len(archive.namelist()), 12)
            self.assertTrue(all(archive.read(name) == self.payload for name in archive.namelist()))


if __name__ == "__main__":
    unittest.main()
