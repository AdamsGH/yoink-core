import json
from pathlib import Path
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit
import socket
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import network_guard as guard
from proxy_tunnel import _resolve_pinned
from content_transport import ContentError, Transport
from content_extract import html_extract, github_route, github_fetch, youtube_fetch, web_fetch, video_id
from content_main import run, validate

FIXTURES = Path(__file__).parent / 'tests' / 'content'

class FixtureTransport:
    def __init__(self, routes):
        self.routes = routes
        self.calls = []
    def get(self, url, token=None):
        self.calls.append((url, token))
        result = self.routes.get(url)
        if result is None:
            raise ContentError('content_http_error')
        if isinstance(result, Exception):
            raise result
        return result
    def json(self, url, token=None):
        return json.loads(self.get(url, token))
    def request(self, url, method='GET', data=None, headers=None):
        raise ContentError('content_http_error')

class ExtractionTests(unittest.TestCase):
    def test_actual_article(self):
        text, title, via = html_extract((FIXTURES / 'article.html').read_text(), 'https://example.com')
        self.assertIn('Extract this real paragraph with & entities.', text)
        self.assertNotIn('noise', text)
        self.assertNotIn('secret', text)
        self.assertEqual(title, 'Fixture article & title')
        self.assertIn(via, ('html-main', 'readability'))

    def test_llms_and_provider_and_raw_fallback(self):
        source = 'https://example.com/doc'
        t = FixtureTransport({'https://example.com/llms.txt': '[Doc](https://example.com/doc.md)', 'https://example.com/doc.md': '# Real markdown\nBody'})
        self.assertEqual(web_fetch(t, source, None), ('# Real markdown\nBody', None, 'llms.txt'))
        t = FixtureTransport({source: '<html><body>Raw body</body></html>', 'https://r.jina.ai/' + source: '---\nurl: x\n---\n' + 'Article prose ' * 30})
        self.assertEqual(web_fetch(t, source, None)[2], 'jina')
        t = FixtureTransport({source: '<html><body>Raw body</body></html>'})
        self.assertEqual(web_fetch(t, source, None), ('Raw body', None, 'raw-html'))

    def test_github_file_readme_issue_and_routes(self):
        base = 'https://api.github.com/repos/owner/repo'
        route = github_route('https://github.com/owner/repo/blob/main/file.txt')
        t = FixtureTransport({route[0]: '{"encoding":"base64","content":"UmVhbCBmaWxl"}'})
        self.assertEqual(github_fetch(t, route, 'readonly')[0], 'Real file')
        t = FixtureTransport({'https://raw.githubusercontent.com/owner/repo/main/file.txt': 'Actual raw file'})
        self.assertEqual(github_fetch(t, route, 'readonly'), ('Actual raw file', None, 'github:raw'))
        self.assertEqual(t.calls, [('https://raw.githubusercontent.com/owner/repo/main/file.txt', None)])
        t = FixtureTransport({base: '{"full_name":"owner/repo","description":"Repository"}', base+'/readme': '{"encoding":"base64","content":"IyBSRUFETUU="}'})
        text, title, via = github_fetch(t, (base, 'repo'), 'readonly')
        self.assertIn('# README', text)
        self.assertEqual(title, 'owner/repo')
        self.assertTrue(all(token == 'readonly' for _, token in t.calls))
        for path, fragment in [('issues/4', '/issues/4'), ('pull/3', '/pulls/3'), ('commits', '/commits?'), ('releases', '/releases?'), ('tree/main', '/git/trees/main')]:
            self.assertIn(fragment, github_route('https://github.com/owner/repo/' + path)[0])
        route = github_route('https://github.com/owner/repo/issues/4')
        t = FixtureTransport({route[0]: '{"number":4,"title":"Bug","body":"Real issue"}', route[0]+'/comments?per_page=20': '[{"body":"Actual comment"}]'})
        self.assertIn('Actual comment', github_fetch(t, route, None)[0])

    def youtube(self, langs, routes=None, only=True):
        t = FixtureTransport({'https://www.youtube.com/watch?v=abcdefghijk': (FIXTURES/'watch.html').read_text(), **(routes or {})})
        return youtube_fetch(t, 'abcdefghijk', langs, only), t

    def test_transcript_priority_and_any_fallback(self):
        result, t = self.youtube(['fr', 'en'], {'https://captions.example/fr': '<transcript><text>Bonjour &amp;amp; monde</text></transcript>'})
        self.assertEqual(result, ('Bonjour & monde', 'Fixture video', 123, 'youtube-transcript:fr'))
        result, t = self.youtube(['en'], {'https://captions.example/en': '<transcript><text>Manual</text></transcript>', 'https://captions.example/en-auto': '<transcript><text>Auto</text></transcript>'})
        self.assertEqual(result[0], 'Manual')
        result, t = self.youtube(['de'], {'https://captions.example/fr': '{"events":[{"segs":[{"utf8":"Fallback"}]}]}'})
        self.assertEqual(result[0], 'Fallback')

    def test_transcript_missing_malformed_description(self):
        result, _ = self.youtube(['en'], only=False)
        self.assertEqual(result[3], 'youtube-description')
        with self.assertRaisesRegex(ContentError, 'no_transcript'):
            self.youtube(['en'], {'https://captions.example/en': 'truncated xml'})
        self.assertEqual(video_id('https://youtu.be/abcdefghijk'), 'abcdefghijk')
        self.assertIsNone(video_id('https://youtube.com.evil/watch?v=abcdefghijk'))

    def test_cap_and_private_source(self):
        req = {'version': 1, 'url': 'https://example.com', 'options': {'max_chars': 3, 'transcript_only': False, 'transcript_languages': [], 'github_token': None}, 'config': {'timeout_secs': 1, 'max_bytes': 1024, 'proxy_url': None}}
        with patch('network_guard.resolve_addresses', return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('93.184.216.34',443))]), patch('content_main.web_fetch', return_value=('😀abcd', 'Title', 'html-main')):
            self.assertEqual(run(req)['text'], '😀ab')
        req['url'] = 'http://127.0.0.1/'
        with self.assertRaises(guard.NetworkPolicyError):
            run(req)
        req['options']['max_chars'] = 0
        with self.assertRaises(ContentError):
            validate(req)

class TransportTests(unittest.TestCase):
    def tearDown(self):
        guard.restore_guard()

    def test_private_mixed_and_rebound_dns(self):
        public = (socket.AF_INET,socket.SOCK_STREAM,6,'',('93.184.216.34',443))
        private = (socket.AF_INET,socket.SOCK_STREAM,6,'',('127.0.0.1',443))
        with patch('network_guard.resolve_addresses', return_value=[public, private]):
            with self.assertRaises(guard.NetworkPolicyError):
                _resolve_pinned('example.com', 443)
        with patch('network_guard.resolve_addresses', side_effect=[[public],[private]]):
            guard.validate_source_url('https://example.com', ())
            with self.assertRaises(guard.NetworkPolicyError):
                _resolve_pinned('example.com', 443)

    def test_real_http_redirect_and_bounds(self):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                if self.path == '/loop':
                    self.send_response(302)
                    self.send_header('Location', '/loop')
                    self.end_headers()
                elif self.path == '/redirect':
                    self.send_response(302)
                    self.send_header('Location', 'http://127.0.0.2/private')
                    self.end_headers()
                else:
                    self.send_response(200)
                    if self.path == '/truncated':
                        self.send_header('Content-Length', '100')
                    self.end_headers()
                    self.wfile.write(b'x' * (2000 if self.path == '/large' else 10))
        server = HTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = 'http://127.0.0.1:' + str(server.server_port)
        try:
            with guard.guard_context((base,)):
                self.assertEqual(Transport(2, 1024).get(base + '/'), 'x'*10)
                with self.assertRaisesRegex(ContentError, 'content_too_large'):
                    Transport(2, 1024).get(base + '/large')
                with self.assertRaises(guard.NetworkPolicyError):
                    Transport(2, 1024).get(base + '/redirect')
                with self.assertRaisesRegex(ContentError, 'content_invalid_response'):
                    Transport(2, 1024).get(base + '/truncated')
                with self.assertRaisesRegex(ContentError, 'content_redirect_limit'):
                    Transport(2, 1024).get(base + '/loop')
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_exact_token_origin_and_redirect_loss(self):
        calls = []
        responses = [(302, 'https://external.example/x'), (302, 'https://api.github.com/repos/a/b'), (200, None)]
        class Response:
            def __init__(self):
                self.status, self.location = responses.pop(0)
            def getheader(self, name):
                return self.location if name == 'Location' else None
            def read(self, amount):
                return b''
        class Connection:
            def __init__(self, host, *a, **kw):
                self.host = host
            def request(self, method, path, body=None, headers=None):
                calls.append((self.host, headers))
            def getresponse(self):
                return Response()
            def close(self):
                pass
        with patch('network_guard.validate_source_url'), patch('content_transport._resolve_pinned', return_value=('93.184.216.34',443)), patch('content_transport.socket.create_connection'), patch('content_transport.ssl.create_default_context'), patch('content_transport.http.client.HTTPConnection', Connection):
            Transport(2,1024).get('https://api.github.com/repos/a/b', 'readonly-secret')
            self.assertEqual(calls[0][1]['Authorization'], 'Bearer readonly-secret')
            self.assertNotIn('Authorization', calls[1][1])
            self.assertNotIn('Authorization', calls[2][1])
            for url in ['https://api.github.com:443/repos/a/b', 'http://api.github.com/repos/a/b', 'https://raw.githubusercontent.com/a/b/main/f', 'https://api.github.com.evil/x']:
                responses.append((200,None))
                Transport(2,1024).get(url, 'readonly-secret')
                self.assertNotIn('Authorization', calls[-1][1])

class ProtocolTests(unittest.TestCase):
    def test_one_shot_real_process_redacts_and_rejects(self):
        import subprocess
        import sys
        req = {'version': 1, 'url': 'http://127.0.0.1/secret', 'options': {'max_chars': 20, 'transcript_only': False, 'transcript_languages': ['en'], 'github_token': 'readonly-secret'}, 'config': {'timeout_secs': 1, 'max_bytes': 1024, 'proxy_url': None}}
        for raw, expected in [(json.dumps(req).encode(), 'content_network_denied'), (b'{truncated', 'content_invalid_response'), (b'x'*32769, 'invalid_request')]:
            child = subprocess.run([sys.executable, '-B', str(Path(__file__).parent/'content_main.py')], input=raw, capture_output=True, timeout=3)
            self.assertEqual(child.returncode, 0)
            self.assertEqual(json.loads(child.stdout), {'version': 1, 'error': expected})
            self.assertEqual(child.stderr, b'')
            self.assertNotIn(b'readonly-secret', child.stdout)
            self.assertNotIn(b'/secret', child.stdout)

    def test_policy_denial_never_falls_back(self):
        denied = guard.NetworkPolicyError('network destination is not permitted')
        t = FixtureTransport({'https://example.com/llms.txt': denied, 'https://r.jina.ai/https://example.com': 'fake successful fallback '*20})
        with self.assertRaises(guard.NetworkPolicyError):
            web_fetch(t, 'https://example.com', None)
        self.assertEqual(len(t.calls), 1)

    def test_proxy_thread_policy_denial_survives_error(self):
        t = Transport(1,1024)
        def relay_failure(*args):
            guard.record_guard_failure()
            raise OSError('redacted relay error')
        with patch.object(t, '_get', side_effect=relay_failure):
            with self.assertRaises(guard.NetworkPolicyError):
                t.get('https://example.com')
