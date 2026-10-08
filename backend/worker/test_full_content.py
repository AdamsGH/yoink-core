import copy
import json
from pathlib import Path
import socket
import subprocess
import sys
import unittest
from unittest.mock import patch
from main import Settings, _validate_request
from full_content import execute, validate
from content_transport import ContentError
from content_test_fetch import FixtureTransport, FIXTURES
import network_guard as guard


def request(url='https://example.com/article'):
    return {'version': 2, 'operation_id': '12345678-1234-1234-1234-123456789abc', 'attempt': 1,
            'task': {'kind': 'full_content_fetch'}, 'url': url,
            'options': {'max_chars': 200000, 'transcript_only': False, 'transcript_languages': ['fr', 'en']},
            'timeout_secs': 2, 'max_response_bytes': 8388608, 'proxy_url': None}


def run(r, settings=None):
    events = []
    execute(r, settings or Settings.from_env({}), events.append,
            {k: r[k] for k in ('version', 'operation_id', 'attempt')})
    assert len(events) == 1
    return events[0]['event']


PUBLIC = [(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('93.184.216.34', 443))]


class FullContentTests(unittest.TestCase):
    def test_youtube_id_matches_shared_query_and_path_cases(self):
        from content_extract import video_id
        cases = json.loads(Path(__file__).with_name('youtube_video_ids.json').read_text())
        for case in cases:
            with self.subTest(url=case['url']):
                self.assertEqual(video_id(case['url']), case['id'])

    def test_strict_dispatch_bounds_and_secrets(self):
        r = request('https://github.com/owner/repo')
        r['options']['github_token'] = 'secret'
        self.assertEqual(_validate_request(json.dumps(r).encode()), r)
        bads = []
        for key, value in [('version', 1), ('attempt', 0), ('operation_id', '00000000-0000-0000-0000-000000000000'), ('timeout_secs', 301), ('max_response_bytes', 1023)]:
            b = copy.deepcopy(r); b[key] = value; bads.append(b)
        for key, value in [('max_chars', 200001), ('transcript_languages', ['en'] * 33), ('transcript_languages', ['en.*']), ('github_token', 'secret\n')]:
            b = copy.deepcopy(r); b['options'][key] = value; bads.append(b)
        for url in ['https://raw.githubusercontent.com/a/b/main/f', 'https://r.jina.ai/https://github.com/a/b', 'https://provider.example/', 'http://api.github.com/a', 'https://api.github.com:444/a']:
            b = copy.deepcopy(r); b['url'] = url; bads.append(b)
        for key in ['network', 'cookie_jar', 'youtube_oauth', 'allowed_private_origins']:
            b = copy.deepcopy(r); b[key] = 'secret'; bads.append(b)
        for b in bads:
            with self.assertRaises((ValueError, ContentError)): validate(b)
        # Anonymous contract must not start accepting the full fetch credentials.
        from content_stage import validate as anonymous_validate
        with self.assertRaises(ValueError): anonymous_validate(r)

    def test_real_main_dispatch_and_private_source_independent_of_media(self):
        for url in ['http://127.0.0.1/secret', 'https://r.jina.ai/http://127.0.0.1/secret']:
            r = request(url)
            child = subprocess.run([sys.executable, '-I', '-B', str(Path(__file__).parent / 'main.py')], input=json.dumps(r).encode(), capture_output=True, timeout=5)
            self.assertEqual(child.returncode, 0)
            self.assertEqual(child.stderr, b'')
            event = json.loads(child.stdout)
            self.assertEqual(event['operation_id'], r['operation_id'])
            self.assertEqual(event['event'], {'type': 'full_content_failed', 'error': 'content_network_denied'})
        settings = Settings.from_env({'YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS': '["http://127.0.0.1"]', 'YOINK_WORKER_ALLOWED_PROXY_ORIGINS': '["http://127.0.0.1"]'})
        self.assertEqual(run(request('http://127.0.0.1'), settings)['error'], 'content_network_denied')

    def test_real_html_and_transcript_engine_through_wrapper(self):
        r = request()
        t = FixtureTransport({'https://example.com/article': (FIXTURES / 'article.html').read_text()})
        with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.Transport', return_value=t):
            event = run(r)
        self.assertIn('Extract this real paragraph', event['result']['text'])
        r = request('https://youtu.be/abcdefghijk'); r['options']['transcript_only'] = True
        base = {'https://www.youtube.com/watch?v=abcdefghijk': (FIXTURES / 'watch.html').read_text()}
        for langs, routes, expected in [
            (['fr', 'en'], {'https://captions.example/fr': '<transcript><text>French</text></transcript>'}, 'French'),
            (['en'], {'https://captions.example/en': '<transcript><text>Manual</text></transcript>', 'https://captions.example/en-auto': '<transcript><text>ASR</text></transcript>'}, 'Manual'),
            (['en'], {'https://captions.example/en-auto': '<transcript><text>ASR</text></transcript>'}, 'ASR'),
            (['de'], {'https://captions.example/fr': '<transcript><text>Any</text></transcript>'}, 'Any')]:
            r['options']['transcript_languages'] = langs
            with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.Transport', return_value=FixtureTransport({**base, **routes})):
                self.assertEqual(run(r)['result']['text'], expected)
        with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.Transport', return_value=FixtureTransport(base)):
            self.assertEqual(run(r)['error'], 'no_transcript')
            r['options']['transcript_only'] = False
            self.assertEqual(run(r)['result']['via'], 'youtube-description')

    def test_multibyte_escaped_single_terminal_and_typed_errors(self):
        r = request()
        for text, expected in [('é' * 200000, 'content_too_large'), ('😀' * 200000, 'content_too_large'), ('\x01' * 200000, 'content_too_large'), ('"\\' * 100000, None)]:
            with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.web_fetch', return_value=(text, None, 'html-main')):
                event = run(r)
                if expected: self.assertEqual(event['error'], expected)
                else: self.assertEqual(event['result']['text'], text)
        for error in ['content_timeout', 'content_too_large', 'content_http_error', 'content_redirect_limit', 'no_content']:
            with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.web_fetch', side_effect=ContentError(error)):
                self.assertEqual(run(r)['error'], error)
        from content_transport import Transport
        with patch.object(Transport, '_get', side_effect=TimeoutError()):
            with self.assertRaisesRegex(ContentError, 'content_timeout'): Transport(1, 1024).get(r['url'])

    def test_provider_nested_source_denial_at_every_hop(self):
        from content_stage import validate_destination
        for url in ['https://r.jina.ai/http://127.0.0.1/a', 'https://curl.md/http://127.0.0.1/a', 'https://r.jina.ai/https://curl.md/http://127.0.0.1/a']:
            with patch('network_guard.resolve_addresses', return_value=PUBLIC), guard.guard_context((), ('http://127.0.0.1',)):
                with self.assertRaises(guard.NetworkPolicyError): validate_destination(url)

    def test_full_github_token_is_not_reused_for_raw_or_provider(self):
        r = request('https://github.com/owner/repo/blob/main/file.txt')
        r['options']['github_token'] = 'readonly-secret'
        provider = 'https://r.jina.ai/' + r['url']
        t = FixtureTransport({provider: 'Actual fallback prose ' * 30})
        with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.Transport', return_value=t):
            self.assertEqual(run(r)['result']['via'], 'jina')
        self.assertTrue(any(url.startswith('https://raw.githubusercontent.com/') for url, _ in t.calls))
        for url, token in t.calls:
            if url.startswith('https://api.github.com/'):
                self.assertEqual(token, 'readonly-secret')
            else:
                self.assertIsNone(token)

    def test_trusted_proxy_permission_is_operational_only(self):
        settings = Settings.from_env({'YOINK_WORKER_ALLOWED_PROXY_ORIGINS': '["http://127.0.0.1:8123"]',
                                      'YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS': '["http://127.0.0.1:8123"]'})
        r = request(); r['proxy_url'] = 'http://127.0.0.1:8123'
        from contextlib import contextmanager
        @contextmanager
        def relay(proxy, timeout):
            self.assertEqual(proxy, r['proxy_url'])
            # Explicit operator permission admits the proxy socket but not source requests.
            with self.assertRaises(guard.NetworkPolicyError): guard.validate_source_url(proxy, ())
            yield 'http://127.0.0.1:8123'
        t = FixtureTransport({'https://example.com/article': (FIXTURES / 'article.html').read_text()})
        with patch('network_guard.resolve_addresses', return_value=PUBLIC), patch('content_main.proxy_route', relay), patch('content_main.Transport', return_value=t):
            self.assertEqual(run(r, settings)['type'], 'full_content_completed')
