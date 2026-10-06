"""Test-only local transport for the real pinned YouTube search extractor.

Only its HTTP transport is remapped; parsing, engine and extraction are real.
Never imported by the production entrypoint.
"""
from __future__ import annotations
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from contextlib import contextmanager
from unittest import mock
from urllib.parse import urlsplit
import yt_dlp


def video(index=0, title=None):
    ident = f"fixture{index:04d}"
    return {"videoRenderer": {"videoId": ident,
        "title": {"runs": [{"text": title or f"Unicode кот {index}"}]},
        "ownerText": {"runs": [{"text": "Fixture channel", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCfixture"}}}]},
        "lengthText": {"simpleText": "1:02"}, "viewCountText": {"simpleText": "1,234 views"},
        "navigationEndpoint": {"watchEndpoint": {"videoId": ident}}}}


def metadata(query):
    count = 0 if query == "zero" else 20 if query == "oversized" else 8
    entries = [video(i) for i in range(count)]
    if query == "duplicates":
        entries = [video(0)] * 8
    if query == "malformed":
        entries = [video(0), {"videoRenderer": {"videoId": "bad"}}, {"channelRenderer": {"channelId": "UCfixture"}}]
    if query == "strings":
        entries = [video(0, "Кот\u202e\n" + "x" * 400)]
    return {"contents": {"twoColumnSearchResultsRenderer": {"primaryContents": {
        "sectionListRenderer": {"contents": [{"itemSectionRenderer": {"contents": entries}}]}}}}}


def player_metadata(ident, origin):
    return {"playabilityStatus": {"status": "OK"}, "videoDetails": {"videoId": ident, "title": "Ordinary inline fixture", "lengthSeconds": "1", "channelId": "UCfixture", "author": "Fixture channel", "shortDescription": "Ordinary fixture", "viewCount": "1234", "isLiveContent": False},
            "microformat": {"playerMicroformatRenderer": {"uploadDate": "2020-01-01", "publishDate": "2020-01-01", "category": "Education"}},
            "streamingData": {"expiresInSeconds": "3600", "formats": [{"itag": 18, "url": origin + "/media.mp4", "mimeType": 'video/mp4; codecs="avc1.42001E, mp4a.40.2"', "bitrate": 200000, "width": 160, "height": 120, "contentLength": "1024", "quality": "medium", "qualityLabel": "144p", "fps": 25, "approxDurationMs": "1000", "audioQuality": "AUDIO_QUALITY_LOW", "audioSampleRate": "44100", "audioChannels": 1}]}}


@contextmanager
def local_extraction(video=False):
    calls = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass
        def do_GET(self):
            if not video or not self.path.startswith("/watch"):
                self.send_error(404)
                return
            player = player_metadata("fixture0000", origin)
            encoded = ('<html><body><script>var ytInitialPlayerResponse = ' + json.dumps(player) + '; var ytInitialData = {"contents":{"twoColumnWatchNextResults":{}}};</script><script>ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB","INNERTUBE_CLIENT_VERSION":"2.20250101.00.00","INNERTUBE_API_KEY":"fixture","STS":20000});</script></body></html>').encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.end_headers()
            try:
                self.wfile.write(encoded)
            except (BrokenPipeError, ConnectionResetError):
                pass
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            query = body.get("query", "")
            calls.append({"path": self.path, "query": query, "cookie": self.headers.get("Cookie")})
            if query == "network denial":
                self.send_response(302)
                self.send_header("Location", "http://127.0.0.2:1/secret")
                self.end_headers()
                return
            encoded = b'x' * (3 * 1024 * 1024) if query == "response overflow" else json.dumps(player_metadata(body.get("videoId", "fixture0000"), origin) if video and self.path.startswith("/youtubei/v1/player") else metadata(query)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            try:
                self.wfile.write(encoded)
            except (BrokenPipeError, ConnectionResetError):
                pass
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    original = yt_dlp.YoutubeDL.urlopen
    def local_urlopen(downloader, request):
        url = urlsplit(request.url)
        if url.hostname not in ("www.youtube.com", "youtube.com") or url.path not in (("/youtubei/v1/search", "/watch", "/youtubei/v1/player") if video else ("/youtubei/v1/search",)):
            raise AssertionError("search attempted a non-metadata request")
        request = request.copy()
        request.url = origin + url.path + ("?" + url.query if url.query else "")
        return original(downloader, request)
    try:
        with mock.patch.object(yt_dlp.YoutubeDL, "urlopen", local_urlopen), \
                mock.patch.object(yt_dlp.YoutubeDL, "process_info", side_effect=AssertionError("media download attempted")):
            yield origin, calls
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    import os
    import sys
    sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
    import main
    raw = sys.stdin.buffer.read(main.MAX_FRAME_BYTES + 1)
    request = main._validate_request(raw)
    with local_extraction() as (origin, _calls):
        settings = main.Settings(1024, 1024, 1, 2, (origin,), ())
        main._search(request, settings)
