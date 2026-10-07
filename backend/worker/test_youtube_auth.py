import io
import json
import os
import socketserver
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from datetime import datetime, timezone
import youtube_auth
from failures import MediaFailure
from content_metadata import MetadataCollector

TOKENS = {"access_token": "synthetic-access", "refresh_token": "synthetic-refresh", "expires_at": "2099-01-01T00:00:00Z"}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_POST(self):
        self.server.requests.append((self.path, json.loads(self.rfile.read(int(self.headers['Content-Length'])))))
        body = self.server.body
        if self.path == '/pot/ping': body = b'{"version":"2.0.0"}'
        if self.path == '/pot': body = b'{"poToken":"synthetic-pot"}'
        if self.path == '/info': body = json.dumps({'title': 'Fixture', 'access_token': 'must-not-escape'}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        if self.server.refresh: self.send_header('x-updated-tokens', 'secret')
        self.end_headers()
        try: self.wfile.write(body)
        except BrokenPipeError: pass

class Tests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = str(Path(self.tmp.name) / 'adapter.sock')
        self.server = socketserver.UnixStreamServer(self.path, Handler)
        self.server.body = b'synthetic-media'
        self.server.refresh = False
        self.server.requests = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.env = patch.dict(os.environ, {youtube_auth.SOCKET_ENV: self.path})
        self.env.start()
    def tearDown(self):
        self.env.stop(); self.server.shutdown(); self.server.server_close(); self.thread.join(); self.tmp.cleanup()
    def test_bounded_actual_unix_download_and_metadata(self):
        metadata = MetadataCollector()
        target = youtube_auth.download({'source_url':'https://youtu.be/abcdefghijk', 'task':{'media_mode':'video'}, 'network':{'youtube_oauth':TOKENS}, 'options':{}}, SimpleNamespace(socket_timeout_secs=2, processing_timeout_secs=2,max_bytes=100,total_max_bytes=100), Path(self.tmp.name), metadata)
        self.assertEqual(target.read_bytes(), b'synthetic-media')
        self.assertEqual(metadata.finish()['title'], 'Fixture')
        self.assertNotIn('must-not-escape', str(metadata.finish()))
        self.assertEqual(self.server.requests[1][1]['tokens']['expiry_date'], TOKENS['expires_at'])
    def test_unix_adapter_does_not_require_internal_origin_exception(self):
        from network_guard import guard_context
        with guard_context((), ()):
            self.assertEqual(youtube_auth.call('/download', {}, 2, 100), b'synthetic-media')

    def test_overflow_stops_stream(self):
        with self.assertRaises(MediaFailure) as error:
            youtube_auth.call('/download', {}, 2, 3, io.BytesIO())
        self.assertEqual(error.exception.failure_class, 'resource_limit')
    def test_refresh_is_not_silently_delivered(self):
        self.server.refresh = True
        with self.assertRaises(MediaFailure): youtube_auth.call('/download', {}, 2, 100)
    def test_import_token_validation_and_redacted_failures(self):
        for bad in [dict(TOKENS, access_token=''), dict(TOKENS, refresh_token='x'*16385), dict(TOKENS, expires_at='bad'), dict(TOKENS, expires_at='2099-01-01')]:
            with self.assertRaises(ValueError): youtube_auth.validate_tokens(bad)
        self.assertGreater(youtube_auth.validate_tokens(TOKENS), datetime.now(timezone.utc))
    def test_remote_socket_paths_are_not_accepted(self):
        with patch.dict(os.environ, {youtube_auth.SOCKET_ENV:'http://private/'}):
            with self.assertRaises(MediaFailure): youtube_auth.call('/info', {}, 1, 100)
    def test_real_oauth_media_processing_clips_and_metadata(self):
        import subprocess
        import media_engine
        generated = Path(self.tmp.name) / 'fixture.mp4'
        subprocess.run(['/usr/bin/ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=64x64:d=1', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', str(generated)], check=True, capture_output=True, timeout=15)
        self.server.body = generated.read_bytes()
        settings = SimpleNamespace(socket_timeout_secs=2, processing_timeout_secs=15, max_bytes=1000000, total_max_bytes=2000000, max_items=2)
        previous = Path.cwd()
        try:
            os.chdir(self.tmp.name)
            with patch('network_guard.validate_source_url'):
                result = media_engine.download_media({'source_url':'https://youtu.be/abcdefghijk', 'task':{'kind':'download', 'media_mode':'video'}, 'network':{'youtube_oauth':TOKENS}, 'options':{'clips':[{'start_seconds':0, 'end_seconds':0.5}]}}, settings, lambda _: None)
            self.assertEqual(len(result), 1)
            self.assertEqual(result[0]['media_type'], 'video/mp4')
            self.assertEqual(result[0]['metadata']['title'], 'Fixture')
            self.assertGreater(result[0]['size_bytes'], 0)
        finally:
            os.chdir(previous)

    def test_pot_provider_registered_and_bound(self):
        import youtube_auth_pot
        youtube_auth_pot.install()
        from yt_dlp.extractor.youtube.pot.provider import _pot_providers
        provider_class = next(p for p in _pot_providers.value.values() if 'YoinkBgutil' in str(p))
        from yt_dlp.extractor.youtube.pot.provider import PoTokenContext
        provider = object.__new__(provider_class)
        result = provider._real_request_pot(SimpleNamespace(internal_client_name='web', context=PoTokenContext.PLAYER, video_id='abcdefghijk', innertube_context={'client':{'clientName':'WEB'}}, bypass_cache=False))
        self.assertEqual(result.po_token, 'synthetic-pot')
        self.assertEqual(self.server.requests[-1][1]['content_binding'], 'abcdefghijk')

if __name__ == '__main__': unittest.main()
