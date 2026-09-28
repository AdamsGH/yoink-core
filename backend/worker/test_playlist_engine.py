"""Real generic-extractor playlists. Run only in the worker container."""
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import shutil
import tempfile
import threading
from types import SimpleNamespace
import unittest

import media_engine


class PlaylistEngineTests(unittest.TestCase):
    def test_selected_playlist_order_survives_artifact_discovery(self):
        media = []
        with tempfile.TemporaryDirectory() as root:
            for color in ("blue", "red"):
                filename = Path(root) / f"{color}.mp4"
                media_engine._bounded_output([
                    shutil.which("ffmpeg"), "-nostdin", "-y", "-loglevel", "error",
                    "-f", "lavfi", "-i", f"color=c={color}:s=64x64:d=0.2",
                    "-c:v", "libx264", "-pix_fmt", "yuv420p", str(filename),
                ], 20)
                media.append(filename.read_bytes())

            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *args):
                    pass

                def do_GET(self):
                    if self.path == "/playlist.html":
                        content = b'<title>Playlist</title><video src="/1.mp4"></video><video src="/2.mp4"></video>'
                        content_type = "text/html"
                    elif self.path in ("/1.mp4", "/2.mp4"):
                        content = media[int(self.path[1]) - 1]
                        content_type = "video/mp4"
                    else:
                        self.send_error(404)
                        return
                    self.send_response(200)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(len(content)))
                    self.end_headers()
                    self.wfile.write(content)

            server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            origin = f"http://127.0.0.1:{server.server_port}"
            settings = SimpleNamespace(socket_timeout_secs=2, processing_timeout_secs=10,
                                       max_bytes=1024 * 1024, total_max_bytes=3 * 1024 * 1024,
                                       max_items=3, allowed_private_origins=[origin])
            try:
                for start, end, expected in ((2, 1, [media[1], media[0]]), (-2, -1, media), (2, None, [media[1]])):
                    with self.subTest(start=start, end=end), tempfile.TemporaryDirectory(dir=root) as work, contextlib.chdir(work):
                        artifacts = media_engine.download_media({
                            "source_url": f"{origin}/playlist.html",
                            "task": {"kind": "download", "media_mode": "video"},
                            "options": {"playlist": {"start": start, "end": end}},
                        }, settings, lambda _: None)
                        self.assertEqual(len(artifacts), len(expected))
                        self.assertTrue([Path(item["path"]).read_bytes() for item in artifacts] == expected,
                                        "artifact order must match the selected playlist order")
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())
