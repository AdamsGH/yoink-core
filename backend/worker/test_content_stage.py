import json
import socket
import subprocess
import sys
import unittest
from pathlib import Path
from importlib.metadata import version
from unittest.mock import patch
from main import Settings
from content_stage import validate, run, validate_destination, MAX_BYTES
import network_guard as guard


def request(task=None):
    return {'version': 2, 'operation_id': '12345678-1234-1234-1234-123456789abc', 'attempt': 1,
            'task': task or {'kind': 'content_get', 'url': 'http://127.0.0.1/secret', 'headers': {}}, 'proxy_url': None}


class StageTests(unittest.TestCase):
    def test_request_bounds_and_credentials(self):
        for key in ('github_token', 'network', 'options'):
            r = request()
            r[key] = 'secret'
            with self.assertRaises(ValueError): validate(r)
        r = request({'kind': 'content_trafilatura', 'html': 'x' * (MAX_BYTES + 1)})
        with self.assertRaises(ValueError): validate(r)
        r = request()
        r['task']['headers']['Authorization'] = 'Bearer secret'
        with self.assertRaises(ValueError): validate(r)
        validate(request({'kind': 'content_trafilatura', 'html': 'x' * MAX_BYTES}))

    def test_private_source_and_both_providers_real_dispatch(self):
        for url in ('http://127.0.0.1/secret', 'https://r.jina.ai/http://127.0.0.1/secret', 'https://curl.md/http://127.0.0.1/secret', 'https://r.jina.ai/https://curl.md/http://127.0.0.1/secret'):
            r = request()
            r['task']['url'] = url
            # Public provider DNS is deterministic; original is still private.
            with patch('network_guard.resolve_addresses', return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('93.184.216.34', 443))]):
                from content_stage import execute
                events = []
                execute(r, Settings.from_env({}), events.append, {'version': 2})
            self.assertEqual(events, [{'version': 2, 'event': {'type': 'content_denied'}}])

    def test_private_dns_original_and_proxy_not_source_allowlist(self):
        with guard.guard_context((), ('http://127.0.0.1:80',)):
            guard.configure_proxy('http://127.0.0.1')
            with patch('network_guard.resolve_addresses', return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('10.0.0.1', 80))]):
                with self.assertRaises(guard.NetworkPolicyError):
                    validate_destination('https://r.jina.ai/http://original.test/a')
            with self.assertRaises(guard.NetworkPolicyError):
                validate_destination('http://127.0.0.1')

    def test_redirect_into_provider_cannot_bypass_original_check(self):
        from content_transport import Transport
        class Response:
            status = 302
            def getheader(self, name):
                return 'https://curl.md/http://127.0.0.1/secret' if name == 'Location' else None
            def read(self, amount): return b''
        class Connection:
            def __init__(self, *a, **k): pass
            def request(self, *a, **k): pass
            def getresponse(self): return Response()
            def close(self): pass
        with guard.guard_context(()), patch('network_guard.resolve_addresses', return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('93.184.216.34', 80))]), patch('content_transport.socket.create_connection'), patch('content_transport.http.client.HTTPConnection', Connection) as conn:
            with self.assertRaises(guard.NetworkPolicyError):
                Transport(1, 1024, validate_destination=validate_destination).get('http://original.test')

    def test_one_shot_denial_redacted(self):
        child = subprocess.run([sys.executable, '-B', str(Path(__file__).parent / 'main.py')], input=json.dumps(request()).encode(), capture_output=True, timeout=3)
        self.assertEqual(child.returncode, 0)
        self.assertEqual(json.loads(child.stdout)['event'], {'type': 'content_denied'})
        self.assertEqual(child.stderr, b'')
        self.assertNotIn(b'/secret', child.stdout)

    def test_transport_pinned_redirect_callback_and_encoding_bounds(self):
        from content_transport import Transport, ContentError
        calls = []
        class Response:
            status = 200
            def getheader(self, name): return 'gzip' if name == 'Content-Encoding' else None
        class Connection:
            def __init__(self, *a, **k): pass
            def request(self, *a, **k): pass
            def getresponse(self): return Response()
            def close(self): pass
        with patch('network_guard._validate_source_request'), patch('content_transport._resolve_pinned', return_value=('93.184.216.34',80)) as pin, patch('content_transport.socket.create_connection'), patch('content_transport.http.client.HTTPConnection', Connection):
            with self.assertRaises(ContentError) as e:
                Transport(1, 1024, validate_destination=calls.append).get('http://example.test')
            self.assertEqual(e.exception.code, 'content_invalid_response')
            pin.assert_called_once_with('example.test', 80)
        self.assertEqual(calls, ['http://example.test'])


class DependencyExtractionTests(unittest.TestCase):
    def test_readability_pinned_dependency(self):
        import readability
        self.assertEqual(version('readability-lxml'), '0.8.4.1')
        html = (Path(__file__).parent / 'tests/content/article.html').read_text()
        result = run(request({'kind': 'content_readability', 'html': html, 'url': 'https://example.com'}), Settings.from_env({}))
        self.assertIn('Extract this real paragraph', result['text'])
        self.assertNotIn('noise', result['text'])
        self.assertIn('Fixture', result['title'])

    def test_trafilatura_pinned_dependency(self):
        import trafilatura
        self.assertEqual(version('trafilatura'), '2.0.0')
        html = (Path(__file__).parent / 'tests/content/article.html').read_text()
        result = run(request({'kind': 'content_trafilatura', 'html': html}), Settings.from_env({}))
        self.assertIn('Extract this real paragraph', result['text'])
        self.assertNotIn('noise', result['text'])
        self.assertNotIn('secret', result['text'])
        # The repair must neither duplicate paragraphs nor discard article text.
        self.assertEqual(result['text'].count('Extract this real paragraph'), 1)
        self.assertIn('Second paragraph.', result['text'])

    def test_trafilatura_semantic_navigation_pruning_preserves_article_prose(self):
        html = (Path(__file__).parent / 'tests/content/article.html').read_text()
        prose = 'Second paragraph about navigation systems is meaningful article prose.'
        for navigation in ('<nav>Navigation noise</nav>', '<div role="navigation">Navigation noise</div>'):
            with self.subTest(navigation=navigation):
                page = html.replace('<nav>Navigation noise</nav>', navigation).replace('Second paragraph.', prose)
                result = run(request({'kind': 'content_trafilatura', 'html': page}), Settings.from_env({}))
                self.assertNotIn('Navigation noise', result['text'])
                self.assertNotIn('Footer noise', result['text'])
                self.assertNotIn('secret script', result['text'])
                self.assertEqual(result['text'].count('Extract this real paragraph'), 1)
                self.assertEqual(result['text'].count(prose), 1)
