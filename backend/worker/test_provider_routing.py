"""Container-only provider transaction and policy regressions; no live provider calls."""
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import media_engine as engine
from failures import MediaFailure


class ProviderRoutingTests(unittest.TestCase):
    def test_failure_policy(self):
        for kind in ('unsupported_source', 'source_unavailable', 'rate_limited', 'invalid_media'):
            self.assertFalse(engine.provider_fallback_allowed(MediaFailure(kind)))
            self.assertTrue(engine.provider_fallback_allowed(MediaFailure(kind, provider_failure=True)))
        for kind in ('resource_limit', 'processing_failed'):
            self.assertFalse(engine.provider_fallback_allowed(MediaFailure(kind)))
        denied = MediaFailure('source_unavailable')
        denied.__cause__ = PermissionError('Landlock filesystem access')
        self.assertFalse(engine.provider_fallback_allowed(denied))
        import asyncio
        cancelled = MediaFailure('source_unavailable')
        cancelled.__cause__ = asyncio.CancelledError()
        self.assertFalse(engine.provider_fallback_allowed(cancelled))
        from yt_dlp.utils import ExtractorError
        import network_guard
        unrelated = engine._classified_provider_failure(ExtractorError('HTTP Error 404 at /p/PRCancel/', expected=True), network_guard.guard_failure_count())
        self.assertTrue(engine.provider_fallback_allowed(unrelated))
        for message in ('network destination is not permitted', 'budget exceeded', 'confinement failure'):
            exc = MediaFailure('source_unavailable')
            exc.__cause__ = OSError(message)
            self.assertFalse(engine.provider_fallback_allowed(exc))

    def test_pinned_provider_json_parse_failure_allows_fallback_without_local_wrappers(self):
        import json
        import network_guard
        from yt_dlp.extractor.common import InfoExtractor
        from yt_dlp.utils import DownloadError, ExtractorError

        with self.assertRaises(ExtractorError) as caught:
            InfoExtractor()._parse_json('{"items":', 'fixture')
        error = caught.exception
        self.assertFalse(error.expected)
        self.assertIsInstance(error.cause, json.JSONDecodeError)
        wrapped = DownloadError('provider wrapper', (type(error), error, None))
        baseline = network_guard.guard_failure_count()
        classified = engine._classified_provider_failure(wrapped, baseline)
        self.assertTrue(engine.provider_fallback_allowed(classified))
        for local in (MediaFailure('invalid_media'), RuntimeError('local runtime error')):
            local.__cause__ = error
            wrapper = ExtractorError('local wrapper', cause=local)
            classified = engine._classified_provider_failure(wrapper, baseline)
            self.assertFalse(engine.provider_fallback_allowed(classified))

    def attempt(self, primary_failure=None, secondary_failure=None):
        calls = []
        def download(request, settings, progress):
            selected = request['options']['engine']
            calls.append(selected)
            Path('output.mp4').write_bytes(selected.encode())
            failure = primary_failure if selected == 'yt_dlp' else secondary_failure
            if failure:
                Path('primary-secret.part').write_text('unpublishable')
                raise failure
            return [{'path': 'output.mp4', 'metadata': {'nsfw': selected == 'gallery'}}]
        request = {'options': {'engine': 'auto_fallback_v1'}}
        with mock.patch.object(engine, 'download_media', side_effect=download):
            result = engine._provider_fallback(request, object(), lambda _: None)
        return calls, result

    def setUp(self):
        self.old = Path.cwd()
        self.temp = tempfile.TemporaryDirectory()
        os.chdir(self.temp.name)

    def tearDown(self):
        os.chdir(self.old)
        self.temp.cleanup()

    def test_primary_success_suppresses_fallback(self):
        calls, result = self.attempt()
        self.assertEqual(calls, ['yt_dlp'])
        self.assertEqual(Path('output.mp4').read_bytes(), b'yt_dlp')
        self.assertFalse(result[0]['metadata']['nsfw'])

    def test_metadata_and_download_provider_failures_are_isolated(self):
        for kind in ('unsupported_source', 'source_unavailable', 'invalid_media'):
            calls, result = self.attempt(MediaFailure(kind, provider_failure=True))
            self.assertEqual(calls, ['yt_dlp', 'gallery'])
            self.assertEqual(Path('output.mp4').read_bytes(), b'gallery')
            self.assertTrue(result[0]['metadata']['nsfw'])
            self.assertEqual([p.name for p in Path.cwd().iterdir()], ['output.mp4'])
            Path('output.mp4').unlink()

    def test_both_fail_no_outputs(self):
        with self.assertRaises(MediaFailure):
            self.attempt(MediaFailure('source_unavailable', provider_failure=True), MediaFailure('invalid_media'))
        self.assertEqual(list(Path.cwd().iterdir()), [])

    def test_limits_and_confinement_do_not_fallback(self):
        for exc in (MediaFailure('resource_limit'), MediaFailure('processing_failed')):
            with self.assertRaises(MediaFailure):
                self.attempt(exc)
            self.assertEqual(list(Path.cwd().iterdir()), [])
        exc = MediaFailure('source_unavailable')
        exc.__cause__ = OSError('network destination is not permitted')
        with self.assertRaises(MediaFailure):
            self.attempt(exc)
        self.assertEqual(list(Path.cwd().iterdir()), [])

    def test_cancellation_is_not_caught(self):
        with self.assertRaises(KeyboardInterrupt):
            self.attempt(KeyboardInterrupt())
        self.assertEqual(list(Path.cwd().iterdir()), [])

class RealProviderRoutingTests(unittest.TestCase):
    """Real installed yt-dlp/directlink gallery extraction over a loopback HTTP source."""
    def setUp(self):
        import subprocess
        import threading
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        from test_gallery_integration import png_fixture
        self.temp = tempfile.TemporaryDirectory()
        self.old = Path.cwd()
        os.chdir(self.temp.name)
        subprocess.run(['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error', '-threads', '1',
                        '-filter_threads', '1', '-f', 'lavfi', '-i', 'color=c=red:s=160x120:r=10',
                        '-f', 'lavfi', '-i', 'sine=frequency=440', '-t', '1', '-c:v', 'libx264',
                        '-c:a', 'aac', '-movflags', '+faststart', 'fixture.mp4'], check=True, timeout=20)
        self.video = Path('fixture.mp4').read_bytes()
        Path('fixture.mp4').unlink()
        self.png = png_fixture()
        self.hits = []
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.hits.append(self.path)
                payload = owner.video if self.path in ('/video.mp4', '/partial.mp4') else owner.png if self.path == '/image.png' else None
                if payload is None:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header('Content-Type', 'video/mp4' if self.path.endswith('.mp4') else 'image/png')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                if self.path == '/partial.mp4':
                    self.wfile.write(payload[:2048])
                    self.wfile.flush()
                    self.close_connection = True
                else:
                    self.wfile.write(payload)
            def log_message(self, *_): pass
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f'http://127.0.0.1:{self.server.server_port}'

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5)
        os.chdir(self.old)
        self.temp.cleanup()

    def download(self, path, route='auto_fallback_v1', mode='video'):
        import main
        settings = main.Settings.from_env({'YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS': '["' + self.origin + '"]'})
        return engine.download_media({'source_url': self.origin + path,
                                      'task': {'kind': 'download', 'media_mode': mode},
                                      'options': {'engine': route}}, settings, lambda _: None)

    def test_real_primary_success_suppresses_gallery(self):
        with mock.patch.object(engine, '_gallery_download', side_effect=AssertionError('fallback called')):
            artifacts = self.download('/video.mp4')
        self.assertEqual(len(artifacts), 1)
        self.assertEqual(artifacts[0]['media_type'], 'video/mp4')
        self.assertEqual(Path(artifacts[0]['path']).read_bytes(), self.video)

    def test_real_metadata_failure_falls_back_to_direct_image(self):
        import yt_dlp
        from yt_dlp.utils import ExtractorError
        with mock.patch.object(yt_dlp.YoutubeDL, 'extract_info', side_effect=ExtractorError('fixture source is unsupported', expected=True)):
            artifacts = self.download('/image.png')
        self.assertEqual(artifacts[0]['media_type'], 'image/png')
        self.assertEqual(Path(artifacts[0]['path']).read_bytes(), self.png)
        self.assertNotIn('image.png', artifacts[0]['metadata']['title'])
        self.assertFalse(any(p.name.startswith('.provider-') for p in Path.cwd().iterdir()))

    def test_real_truncated_download_retains_typed_provider_cause(self):
        from failures import exception_chain
        with self.assertRaises(MediaFailure) as failure:
            self.download('/partial.mp4', 'yt_dlp')
        self.assertEqual(failure.exception.failure_class, 'source_unavailable',
                         [(type(item).__name__, getattr(item, 'fallback_forbidden', None)) for item in exception_chain(failure.exception)])
        self.assertFalse(failure.exception.fallback_forbidden,
                         [type(item).__name__ for item in exception_chain(failure.exception)])
        from yt_dlp.utils import ContentTooShortError, DownloadError
        from yt_dlp.networking.exceptions import TransportError
        self.assertTrue(any(isinstance(item, (ContentTooShortError, TransportError)) for item in exception_chain(failure.exception)))
        self.assertTrue(any(isinstance(item, DownloadError) and isinstance(getattr(item, 'provider_cause', None), (ContentTooShortError, TransportError))
                            for item in exception_chain(failure.exception)))

    def test_real_both_fail_no_artifacts(self):
        with self.assertRaises(MediaFailure):
            self.download('/missing.mp4')
        self.assertEqual(list(Path.cwd().iterdir()), [])

    def test_real_gallery_accepts_video_and_explicit_image_keeps_video(self):
        for mode in ('video', 'image'):
            artifacts = self.download('/video.mp4', 'auto_gallery_v1', mode)
            self.assertEqual(artifacts[0]['media_type'], 'video/mp4')
            self.assertEqual(Path(artifacts[0]['path']).read_bytes(), self.video)
            Path(artifacts[0]['path']).unlink()

class StructuralFallbackTests(unittest.TestCase):
    """Default-deny setup/callback/guard errors, exercised through the real orchestrator."""
    def setUp(self):
        from types import SimpleNamespace
        self.old = Path.cwd()
        self.temp = tempfile.TemporaryDirectory()
        os.chdir(self.temp.name)
        self.settings = SimpleNamespace(socket_timeout_secs=1, processing_timeout_secs=3,
                                        max_bytes=1000, total_max_bytes=2000, max_items=3,
                                        allowed_private_origins=())
        self.request = {'source_url': 'https://provider.example/video',
                        'task': {'kind': 'download', 'media_mode': 'video'},
                        'options': {'engine': 'auto_fallback_v1'}}

    def tearDown(self):
        os.chdir(self.old)
        self.temp.cleanup()

    def assert_no_fallback(self, request=None):
        with mock.patch.object(engine, '_gallery_download') as gallery:
            with self.assertRaises(MediaFailure) as failure:
                engine.download_media(request or self.request, self.settings, lambda _: None)
            self.assertTrue(failure.exception.fallback_forbidden)
            self.assertFalse(engine.provider_fallback_allowed(failure.exception))
            gallery.assert_not_called()
        self.assertEqual(list(Path.cwd().iterdir()), [])
        return failure.exception

    def fake_downloader(self, action):
        class Downloader:
            def __init__(self, options): self.options = options
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def download(self, _urls): return action(self.options)
        return Downloader

    def test_exact_source_validation_errors_never_enter_an_extractor(self):
        import network_guard
        import yt_dlp
        for source in ('ftp://provider.example/file', 'http://127.0.0.1/file',
                       'https://user:password@provider.example/file', 'https://provider.example:bad/file',
                       'https://provider.example/\x01file'):
            with self.subTest(source=source), mock.patch.object(yt_dlp, 'YoutubeDL') as primary:
                self.assert_no_fallback({**self.request, 'source_url': source})
                primary.assert_not_called()
        self.settings.allowed_private_origins = ('not-an-origin',)
        self.assert_no_fallback({**self.request, 'source_url': 'http://127.0.0.1/file'})
        self.settings.allowed_private_origins = ()
        import socket
        public = (socket.AF_INET, socket.SOCK_STREAM, 6, '', ('8.8.8.8', 443))
        private = (socket.AF_INET, socket.SOCK_STREAM, 6, '', ('127.0.0.1', 443))
        for answers in ([], [public, private]):
            with mock.patch.object(network_guard, '_ORIGINAL_GETADDRINFO', return_value=answers):
                self.assert_no_fallback()
        with mock.patch.object(network_guard, '_ORIGINAL_GETADDRINFO', side_effect=socket.gaierror('DNS failed')):
            self.assert_no_fallback()

    def test_tools_cookie_and_collector_setup_errors_are_structurally_forbidden(self):
        import network_guard
        with mock.patch.object(network_guard, 'validate_source_url'):
            for failure in (MediaFailure('unsupported_source'), RuntimeError('unexpected tool setup')):
                with mock.patch.object(engine, '_tools', side_effect=failure):
                    self.assert_no_fallback()
            with mock.patch.object(engine, 'MetadataCollector', side_effect=RuntimeError('collector setup')):
                self.assert_no_fallback()
            for failure in (OSError('local cookie storage'), RuntimeError('unexpected cookie setup')):
                with mock.patch.object(engine.tempfile, 'mkstemp', side_effect=failure):
                    self.assert_no_fallback({**self.request, 'network': {'cookie_jar': '# Netscape\n'}})

    def test_constructor_and_unexpected_extractor_implementation_errors_fail_closed(self):
        import network_guard
        import yt_dlp
        from yt_dlp.utils import DownloadError, ExtractorError
        with mock.patch.object(network_guard, 'validate_source_url'):
            with mock.patch.object(yt_dlp, 'YoutubeDL', side_effect=RuntimeError('local runtime setup')):
                self.assert_no_fallback()
            for failure in (RuntimeError('unexpected implementation'), TypeError('implementation bug'),
                            ExtractorError('implementation bug', expected=False),
                            DownloadError('opaque failure without a provider cause')):
                def action(_options, failure=failure): raise failure
                with mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)):
                    self.assert_no_fallback()
            with mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(lambda _: 1)):
                self.assert_no_fallback()

    def test_local_metadata_and_progress_callbacks_cannot_become_provider_failures(self):
        import network_guard
        import yt_dlp
        from yt_dlp.utils import DownloadError
        for callback in ('match_filter', 'progress_hooks'):
            def action(options, callback=callback):
                try:
                    if callback == 'match_filter':
                        options[callback]({'title': 'fixture'})
                    else:
                        options[callback][0]({'status': 'finished', 'info_dict': {'title': 'fixture'}})
                except MediaFailure as exc:
                    # Exact pinned DownloadError storage; even without Python __cause__ links.
                    raise DownloadError('extractor wrapper', (type(exc), exc, None)) from None
            with mock.patch.object(network_guard, 'validate_source_url'), \
                    mock.patch.object(engine.MetadataCollector, 'add', side_effect=RuntimeError('local callback')), \
                    mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)):
                self.assert_no_fallback()

    def test_guard_denials_in_library_specific_links_and_erased_thread_errors_forbid_fallback(self):
        import network_guard
        import yt_dlp
        from yt_dlp.networking.exceptions import TransportError
        from yt_dlp.utils import DownloadError, ExtractorError
        for mode in ('cause', 'exc_info', 'erased-relay'):
            def action(_options, mode=mode):
                # Arbitrary diagnostic text deliberately lacks every formerly matched keyword.
                denied = network_guard.NetworkPolicyError('opaque guard rejection')
                if mode == 'cause':
                    raise TransportError(cause=denied) from None
                if mode == 'exc_info':
                    raise DownloadError('HTTP Error 503', (type(denied), denied, None)) from None
                # A relay thread converts policy/local failures to HTTP, erasing exception links.
                raise ExtractorError('HTTP Error 502', expected=True) from None
            with mock.patch.object(network_guard, 'validate_source_url'), \
                    mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)):
                self.assert_no_fallback()

    def test_guard_validation_during_extraction_is_not_a_provider_failure(self):
        import network_guard
        import yt_dlp
        def action(_options):
            network_guard._parse_http_url('file:///local', 'opaque parse rejection')
        with mock.patch.object(network_guard, 'validate_source_url'), \
                mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)):
            self.assert_no_fallback()

    def test_local_artifact_processing_stays_forbidden_after_successful_extraction(self):
        import network_guard
        import yt_dlp
        def action(_options):
            Path('source-fixture.mp4').write_bytes(b'not actual media')
            return 0
        with mock.patch.object(network_guard, 'validate_source_url'), \
                mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)), \
                mock.patch.object(engine, '_probe', side_effect=RuntimeError('unexpected local probe')):
            self.assert_no_fallback()

    def test_every_network_guard_policy_path_has_a_structural_marker(self):
        import network_guard
        import proxy_tunnel
        import socket
        public = (socket.AF_INET, socket.SOCK_STREAM, 6, '', ('8.8.8.8', 443))
        private = (socket.AF_INET, socket.SOCK_STREAM, 6, '', ('127.0.0.1', 443))
        with network_guard.guard_context(()):
            for answers in ([], [public, private], [(None,)]):
                with mock.patch.object(network_guard, '_ORIGINAL_GETADDRINFO', return_value=answers):
                    baseline = network_guard.guard_failure_count()
                    with self.assertRaises(network_guard.NetworkPolicyError) as denied:
                        network_guard.guarded_getaddrinfo('provider.example', 443)
                    self.assertTrue(denied.exception.fallback_forbidden)
                    self.assertGreater(network_guard.guard_failure_count(), baseline)
            for call in (lambda: network_guard._validate_proxy_request('http://8.8.8.8:8080'),
                         lambda: network_guard.configure_proxy('http://127.0.0.1:8080'),
                         lambda: proxy_tunnel._resolve_pinned('127.0.0.1', 443)):
                baseline = network_guard.guard_failure_count()
                with self.assertRaises((network_guard.NetworkPolicyError, network_guard.NetworkValidationError)) as denied:
                    call()
                self.assertTrue(denied.exception.fallback_forbidden)
                self.assertGreater(network_guard.guard_failure_count(), baseline)

    def test_expected_provider_wrappers_cannot_hide_local_runtime_or_enforcement_failure(self):
        import network_guard
        import yt_dlp
        from yt_dlp.utils import DownloadError, ExtractorError
        for underlying in (RuntimeError('local implementation bug'), MediaFailure('invalid_media')):
            # Even an expected extractor wrapper must not erase a local enforcement cause.
            wrapped = ExtractorError('provider diagnostic', expected=True, cause=underlying)
            error = DownloadError('extractor wrapper', (type(wrapped), wrapped, None))
            def action(_options, error=error): raise error
            with mock.patch.object(network_guard, 'validate_source_url'), \
                    mock.patch.object(yt_dlp, 'YoutubeDL', self.fake_downloader(action)):
                self.assert_no_fallback()
