from email.message import Message
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import unittest
from unittest.mock import patch

from content_extract import youtube_fetch, youtube_transcript
from content_test_fetch import FixtureTransport, FIXTURES
from content_transport import ContentError, Transport
import network_guard as guard
from youtube_captions import CaptionDL, caption_tracks
from yt_dlp.networking import Request
from yt_dlp.utils import DownloadError


VIDEO = 'abcdefghijk'
URL = 'https://www.youtube.com/api/timedtext?v=' + VIDEO + '&sig=opaque&fmt=json3'
TRACK = {'baseUrl': URL, 'languageCode': 'en'}


class CaptionTests(unittest.TestCase):
    def test_empty_native_captions_use_sdk_url_without_rewriting(self):
        t = FixtureTransport({'https://www.youtube.com/watch?v=' + VIDEO: (FIXTURES / 'watch.html').read_text(),
                              URL: json.dumps({'events': [{'segs': [{'utf8': 'Full speech 😀'}]}]})})
        with patch('content_extract.caption_tracks', return_value=[TRACK]) as sdk:
            result = youtube_fetch(t, VIDEO, ['en'], True)
        self.assertEqual(result[0], 'Full speech 😀')
        self.assertEqual(result[3], 'youtube-transcript:en')
        self.assertIn((URL, None), t.calls)
        sdk.assert_called_once_with(t, VIDEO)

    def test_empty_malformed_and_language_order(self):
        for raw in ('', '{bad', '{"events":[{"segs":[{"utf8":7}]}]}', '<bad>'):
            with self.subTest(raw=raw):
                self.assertIsNone(youtube_transcript(FixtureTransport({URL: raw}), [TRACK], ['en']))
        tracks = [{'baseUrl': URL + '&kind=asr', 'languageCode': 'en', 'kind': 'asr'}, TRACK,
                  {'baseUrl': URL + '&lang=fr', 'languageCode': 'fr'}]
        t = FixtureTransport({tracks[0]['baseUrl']: '<text>ASR</text>', URL: '<text>Manual</text>',
                              tracks[2]['baseUrl']: '<text>French</text>'})
        self.assertEqual(youtube_transcript(t, tracks, ['fr', 'en']), ('French', 'fr'))
        self.assertEqual(youtube_transcript(t, tracks, ['en']), ('Manual', 'en'))

    def test_empty_sdk_and_critical_failures(self):
        t = FixtureTransport({'https://www.youtube.com/watch?v=' + VIDEO: (FIXTURES / 'watch.html').read_text()})
        with patch('content_extract.caption_tracks', return_value=[]):
            with self.assertRaisesRegex(ContentError, 'no_transcript'):
                youtube_fetch(t, VIDEO, ['en'], True)
            self.assertEqual(youtube_fetch(t, VIDEO, ['en'], False)[3], 'youtube-description')
        for error in (ContentError('content_timeout'), ContentError('content_too_large'),
                      guard.NetworkPolicyError('denied')):
            with patch('content_extract.caption_tracks', side_effect=error):
                with self.assertRaises(type(error)):
                    youtube_fetch(t, VIDEO, ['en'], True)

    def test_sdk_original_language_and_actor_local_cookie_jar(self):
        info = {'id': VIDEO, 'subtitles': {'en': [{'ext': 'json3', 'url': URL}]},
                'automatic_captions': {'ru-orig': [{'ext': 'json3', 'url': URL + '&lang=ru'}],
                                       'fr': [{'ext': 'json3', 'url': URL + '&tlang=fr'}]}}
        with patch.object(CaptionDL, 'extract_info', return_value=info):
            self.assertEqual(caption_tracks(Transport(1, 1024), VIDEO),
                             [dict(TRACK, kind=''), {'baseUrl': URL + '&lang=ru', 'languageCode': 'ru', 'kind': 'asr'}])
        with CaptionDL(Transport(1, 1024)) as first, CaptionDL(Transport(1, 1024)) as second:
            self.assertIsNot(first.cookiejar, second.cookiejar)
            self.assertEqual(first.params['extractor_args']['youtube']['fetch_pot'], ['never'])
            self.assertFalse(first.params['nocheckcertificate'])
            self.assertFalse(first.params['cachedir'])

    def test_wrapped_guard_denial_is_not_no_transcript(self):
        def denied(*args, **kwargs):
            guard.record_guard_failure()
            raise DownloadError('redacted')
        with patch.object(CaptionDL, 'extract_info', side_effect=denied):
            with self.assertRaises(guard.NetworkPolicyError):
                caption_tracks(Transport(1, 1024), VIDEO)

    def test_sdk_bridge_uses_transport_and_session_cookies(self):
        headers = Message()
        headers['Set-Cookie'] = 'visitor=operation-local; Path=/; Secure'
        t = Transport(1, 1024)
        with patch.object(t, 'request', return_value=(b'{}', 'https://www.youtube.com/watch', headers, 200)) as send:
            with CaptionDL(t) as client:
                self.assertEqual(client.urlopen(Request('https://www.youtube.com/watch')).read(), b'{}')
                self.assertIn('visitor=operation-local', client.cookiejar.get_cookie_header('https://www.youtube.com/watch'))
                client.urlopen(Request('https://www.youtube.com/watch', data=b'{}'))
                self.assertEqual(send.call_args.args[1:3], ('POST', b'{}'))
                self.assertIn('visitor=operation-local', send.call_args.args[3]['Cookie'])


class RawTransportTests(unittest.TestCase):
    def test_real_post_redirect_bounds_headers_and_private_denial(self):
        calls = []
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_POST(self):
                calls.append((self.command, self.headers.get('Cookie')))
                self.rfile.read(int(self.headers.get('Content-Length', 0)))
                self.send_response(303)
                self.send_header('Location', '/end')
                self.send_header('Content-Length', '0')
                self.end_headers()
            def do_GET(self):
                calls.append((self.command, self.headers.get('Cookie')))
                self.send_response(200)
                self.send_header('Content-Length', '2048' if self.path == '/large' else '2')
                self.send_header('Set-Cookie', 'a=one; Path=/')
                self.send_header('Set-Cookie', 'b=two; Path=/')
                self.end_headers()
                self.wfile.write(b'{}')
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        origin = 'http://127.0.0.1:' + str(server.server_port)
        try:
            with guard.guard_context((origin,), ()):
                body, url, headers, status = Transport(2, 1024).request(origin + '/start', 'POST', b'{}', {'Cookie': 'owned'})
                self.assertEqual((body, url, status), (b'{}', origin + '/end', 200))
                self.assertEqual(headers.get_all('Set-Cookie'), ['a=one; Path=/', 'b=two; Path=/'])
                self.assertEqual(calls, [('POST', 'owned'), ('GET', 'owned')])
                with self.assertRaisesRegex(ContentError, 'content_too_large'):
                    Transport(2, 1024).request(origin + '/large')
                t = Transport(2, 1024)
                t.deadline = 0
                with self.assertRaisesRegex(ContentError, 'content_timeout'):
                    t.request(origin + '/end')
            with guard.guard_context((), ()):
                with self.assertRaises(guard.NetworkPolicyError):
                    Transport(2, 1024).request(origin + '/end')
        finally:
            server.shutdown()
            server.server_close()
            thread.join(2)


if __name__ == '__main__':
    unittest.main()
