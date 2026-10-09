"""Offline Music fixtures. Run only in the pinned worker container."""
import asyncio
from contextlib import contextmanager
import importlib
import os
from pathlib import Path
import subprocess
import socket
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

import httpx
import music_engine
from music_library import transport
from music_library.types import ResolverError

PUBLIC_DNS = [(socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP, "", ("93.184.216.34", 443))]

class Client:
    def __init__(self, response=None):
        self.response = response
    async def __aenter__(self):
        return self
    async def __aexit__(self, *args):
        pass
    async def get(self, *args, **kwargs):
        return self.response

def response(body=None, text=None):
    return httpx.Response(200, json=body, text=text, request=httpx.Request("GET", "https://example.com/fixture"))

class MusicProviderTests(unittest.IsolatedAsyncioTestCase):
    async def test_all_provider_dispatch_signatures_and_empty_metadata(self):
        credentials = {key: None for key in ("spotify_client_id", "spotify_client_secret", "tidal_client_id", "tidal_client_secret")}
        with patch.object(transport, "AsyncClient", return_value=Client()):
            for provider in music_engine.PROVIDERS:
                module = importlib.import_module("music_library.parsers." + provider)
                mock = AsyncMock(return_value=("Song", "Artist", None))
                with self.subTest(provider=provider), patch.object(module, "parse", mock):
                    result = await music_engine.provider_task({"kind": "music_parse", "provider": provider, "url": "https://example.com/fixture"}, credentials, None)
                    self.assertEqual(result["metadata"]["title"], "Song")
                    args, kwargs = mock.call_args
                    self.assertEqual(len(args), 1 if provider == "soundcloud" else 2)
                    if provider == "spotify":
                        self.assertIn("client_secret", kwargs)
                    if provider == "tidal":
                        self.assertNotIn("client_secret", kwargs)
                    mock.return_value = ("", "Artist", None)
                    with self.assertRaises(music_engine.MediaFailure):
                        await music_engine.provider_task({"kind": "music_parse", "provider": provider, "url": "https://example.com/fixture"}, credentials, None)

    async def test_public_html_parsers_and_anonymous_api_fixtures(self):
        for provider, title in [("apple_music", "Song by Artist on Apple Music"), ("bandcamp", "Song, by Artist"), ("qobuz", "Song, Artist - Qobuz"), ("tidal", "Artist - Song")]:
            module = importlib.import_module("music_library.parsers." + provider)
            html = '<meta property="og:title" content="' + title + '"><meta property="og:image" content="https://example.com/cover.jpg">'
            with self.subTest(provider=provider):
                self.assertEqual(await module.parse("https://example.com/song", Client(response(text=html))), ("Song", "Artist", "https://example.com/cover.jpg"))
        from music_library.parsers import deezer, yandex
        self.assertEqual((await deezer.parse("https://www.deezer.com/track/42", Client(response({"title": "Song", "artist": {"name": "Artist"}, "album": {"cover_medium": "https://example.com/image"}}))))[:2], ("Song", "Artist"))
        self.assertEqual(await yandex.parse("https://music.yandex.ru/album/1/track/2", Client(response({"result": [{"title": "Song", "artists": [{"name": "Artist"}], "coverUri": "example.com/%%"}]}))), ("Song", "Artist", "https://example.com/400x400"))

    async def test_youtube_music_category_and_title_fallback(self):
        from music_library.parsers import youtube
        with patch.object(youtube, "_run_ytdlp", return_value={"title": "Artist - Song (Official Video)", "categories": ["Music"], "channel": "Artist - Topic"}):
            result = await youtube.parse("https://www.youtube.com/watch?v=abcdefghijk", Client())
            self.assertEqual(result[:2], ("Song", "Artist"))
        with patch.object(youtube, "_run_ytdlp", return_value={"title": "Not music", "categories": ["Gaming"]}):
            with self.assertRaises(ResolverError):
                await youtube.parse("https://www.youtube.com/watch?v=abcdefghijk", Client())

    async def test_spotify_artist_top_five_contract(self):
        from music_library.parsers import artist
        from music_library.types import ArtistInfo, ArtistTrack
        info = ArtistInfo("Artist", ["rock"], None, "https://open.spotify.com/artist/abc", [("spotify", "Spotify", "https://open.spotify.com/artist/abc")], [ArtistTrack("Song", [("spotify", "Spotify", "https://open.spotify.com/track/abc")])])
        with patch.object(transport, "AsyncClient", return_value=Client()), patch.object(artist, "resolve_spotify_artist", AsyncMock(return_value=info)):
            result = await music_engine.provider_task({"kind": "music_artist", "url": info.source_url}, {"spotify_client_id": "fixture", "spotify_client_secret": "fixture"}, None)
            self.assertEqual(result["kind"], "artist")
            self.assertNotIn("source_url", result)
            self.assertEqual(result["top_tracks"][0]["title"], "Song")

    async def test_http_decoded_response_budget(self):
        class FakeTransport:
            async def handle_async_request(self, request):
                return httpx.Response(200, content=b"x" * 2048, request=request)
        transport.configure(1, 1024, 2)
        bounded = transport.BoundedTransport()
        real = bounded.inner
        bounded.inner = FakeTransport()
        try:
            with patch.object(transport._budget, "admit"), self.assertRaises(music_engine.MediaFailure):
                await bounded.handle_async_request(httpx.Request("GET", "https://example.com/fixture"))
        finally:
            await real.aclose()

    async def test_soundcloud_bundle_exhaustion_does_no_more_dns(self):
        from music_library.parsers import soundcloud
        transport.configure(1, 8192, 1)
        html = "".join(f'<script src="https://a-v2.sndcdn.com/bundle-{index}.js"></script>' for index in range(40))
        class BudgetClient(Client):
            async def get(self, url, **kwargs):
                transport._budget.admit(url)
                return response(text=html)
        with patch.object(soundcloud, "_make_session", return_value=BudgetClient()), patch.object(soundcloud._cache, "version", ""), patch.object(soundcloud._cache, "client_id", ""), patch.object(transport.guard, "_validate_source_request") as validate:
            self.assertIsNone(await soundcloud._fetch_client_id(None))
            self.assertEqual(validate.call_count, 1)

class MusicSecurityAndAudioTests(unittest.TestCase):
    @contextmanager
    def urllib_wire(self, downloader, responses):
        import io
        from email.message import Message
        from urllib.request import BaseHandler
        from urllib.response import addinfourl
        calls = []
        class Wire(BaseHandler):
            handler_order = 0
            def https_open(self, request):
                calls.append((request.full_url, request.timeout))
                status, headers, body = responses[len(calls) - 1]
                message = Message()
                for key, value in headers.items():
                    message[key] = value
                result = addinfourl(io.BytesIO(body), message, request.full_url, status)
                result.msg = "fixture"
                return result
            http_open = https_open
        handler = downloader._request_director.handlers["Urllib"]
        create = handler._create_instance
        def create_with_wire(*args, **kwargs):
            opener = create(*args, **kwargs)
            opener.add_handler(Wire())
            return opener
        with patch.object(handler, "_create_instance", side_effect=create_with_wire):
            yield calls

    def test_private_sources_and_request_budget_are_rejected(self):
        for url in ("http://127.0.0.1/song", "http://169.254.169.254/latest/meta-data", "http://[::1]/song"):
            transport.configure(1, 1024, 1)
            with self.subTest(url=url), self.assertRaises(Exception):
                transport._budget.admit(url)
        transport.configure(1, 1024, 1)
        with patch.object(transport.guard, "_validate_source_request"):
            transport._budget.admit("https://example.com/song")
            with self.assertRaises(music_engine.MediaFailure):
                transport._budget.admit("https://example.com/song")

    def test_exhausted_requests_do_no_dns_even_when_repeated(self):
        transport.configure(1, 1024, 1)
        with patch.object(transport.guard, "_validate_source_request") as validate:
            transport._budget.admit("https://example.com/first")
            for index in range(100):
                with self.assertRaises(music_engine.MediaFailure):
                    transport._budget.admit(f"https://example.com/bundle-{index}.js")
            self.assertEqual(validate.call_count, 1)
            self.assertEqual(transport._budget.requests, 0)

    def test_urllib_audio_timeout_and_request_admission(self):
        from music_library.ytdlp import AudioYoutubeDL
        transport.configure(0.25, 1024, 1)
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "socket_timeout": 30, "proxy": ""}) as downloader:
            self.assertEqual(downloader.params["socket_timeout"], 0.25)
            self.assertEqual(set(downloader._request_director.handlers), {"Urllib"})
            with self.urllib_wire(downloader, [(200, {}, b"x" * 4096)]) as calls, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
                with downloader.urlopen("https://example.com/audio") as streamed:
                    self.assertEqual(len(streamed.read()), 4096)
                self.assertEqual(calls[0][1], 0.25)
                with self.assertRaises(music_engine.MediaFailure):
                    downloader.urlopen("https://example.com/second")
                self.assertEqual(len(calls), 1)
                self.assertEqual(dns.call_count, 1)

    def test_extractor_bodies_are_bounded_but_streams_are_not(self):
        from yt_dlp.extractor.common import InfoExtractor
        from music_library.ytdlp import AudioYoutubeDL
        class FixtureIE(InfoExtractor):
            def _request_webpage(self, url, video_id):
                return self._downloader.urlopen(url)
        transport.configure(0.25, 1024, 3)
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "socket_timeout": 30, "proxy": ""}) as downloader, self.urllib_wire(downloader, [(200, {}, b"x" * 4096)] * 3), patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS):
            ie = FixtureIE()
            downloader.add_info_extractor(ie)
            with ie._request_webpage("https://example.com/metadata", "fixture") as body:
                with self.assertRaises(music_engine.MediaFailure):
                    body.read()
                self.assertTrue(body.closed)
            with ie._request_webpage("https://example.com/chunked-metadata", "fixture") as body:
                self.assertEqual(len(body.read(1024)), 1024)
                with self.assertRaises(music_engine.MediaFailure):
                    body.read(1)
            with downloader.urlopen("https://example.com/audio") as body:
                self.assertEqual(len(body.read()), 4096)

    def test_requests_redirects_charge_each_wire_before_dns(self):
        import io
        import requests
        for maximum in (1, 2):
            calls = []
            class Wire(requests.adapters.HTTPAdapter):
                def send(self, request, **kwargs):
                    calls.append((request.url, kwargs))
                    result = requests.Response()
                    result.status_code = 302 if len(calls) == 1 else 200
                    if len(calls) == 1:
                        result.headers["Location"] = "/final"
                    result.url, result.request = request.url, request
                    result.raw = io.BytesIO(b"fixture")
                    return result
            transport.configure(0.25, 1024, maximum)
            with self.subTest(maximum=maximum), transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), transport.requests_budget(), requests.Session() as session, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
                session.mount("https://", Wire(max_retries=3))
                if maximum == 1:
                    with self.assertRaises(music_engine.MediaFailure):
                        session.get("https://example.com/start")
                else:
                    self.assertEqual(session.get("https://example.com/start").content, b"fixture")
                self.assertEqual(dns.call_count, maximum)
                self.assertEqual(len(calls), maximum)
                self.assertEqual(transport._budget.requests, 0)
                self.assertTrue(all(kwargs["timeout"] == 0.25 for _, kwargs in calls))
                self.assertEqual(session.get_adapter("https://example.com").max_retries.total, 0)

    def test_urllib_redirects_budget_extraction_and_streaming(self):
        from music_library.ytdlp import AudioYoutubeDL
        from yt_dlp.extractor.common import InfoExtractor
        for maximum, extraction in ((1, False), (2, False), (2, True)):
            transport.configure(0.25, 1024, maximum)
            with self.subTest(maximum=maximum, extraction=extraction), transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "proxy": ""}) as downloader, self.urllib_wire(downloader, [(302, {"Location": "https://example.com/final"}, b""), (200, {}, b"x" * 4096)]) as calls, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
                ie = InfoExtractor()
                downloader.add_info_extractor(ie)
                if maximum == 1:
                    with self.assertRaises(music_engine.MediaFailure):
                        downloader.urlopen("https://example.com/start")
                elif extraction:
                    with ie._request_webpage("https://example.com/start", "fixture") as body:
                        self.assertEqual(len(body.read(1024)), 1024)
                        with self.assertRaises(music_engine.MediaFailure):
                            body.read(1)
                        self.assertTrue(body.closed)
                else:
                    with downloader.urlopen("https://example.com/start") as body:
                        self.assertEqual(len(body.read()), 4096)
                self.assertEqual(dns.call_count, maximum)
                self.assertEqual(len(calls), maximum)
                self.assertEqual(transport._budget.requests, 0)
                self.assertTrue(all(timeout == 0.25 for _, timeout in calls))

    def test_requests_redirect_body_is_bounded_before_redirect_processing(self):
        import io
        import requests
        class Wire(requests.adapters.HTTPAdapter):
            calls = 0
            def send(self, request, **kwargs):
                self.calls += 1
                response = requests.Response()
                response.status_code = 302
                response.url, response.request = request.url, request
                response.headers["Location"] = "/final"
                response.raw = io.BytesIO(b"x" * 4096)
                return response
        transport.configure(0.25, 1024, 2)
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), transport.requests_budget(), requests.Session() as session, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
            wire = Wire()
            session.mount("https://", wire)
            with self.assertRaises(music_engine.MediaFailure):
                session.get("https://example.com/start")
            self.assertEqual(wire.calls, 1)
            self.assertEqual(dns.call_count, 1)
            self.assertEqual(transport._budget.requests, 1)

    def test_music_rejects_uncovered_handlers_without_wire_or_dns(self):
        from music_library.ytdlp import AudioYoutubeDL, YoutubeDL
        from yt_dlp.networking.common import Request
        from yt_dlp.networking.exceptions import RequestError
        for factory in (AudioYoutubeDL, YoutubeDL):
            transport.configure(0.25, 1024, 1)
            with self.subTest(factory=factory), transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), factory({"quiet": True, "proxy": ""}) as downloader, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
                self.assertEqual(set(downloader._request_director.handlers), {"Urllib"})
                with self.assertRaises(RequestError):
                    downloader.urlopen(Request("https://example.com/fixture", extensions={"impersonate": object()}))
                self.assertEqual(dns.call_count, 0)
                self.assertEqual(transport._budget.requests, 1)

    def test_urllib_extraction_redirect_body_is_bounded_before_next_wire(self):
        from music_library.ytdlp import AudioYoutubeDL
        from yt_dlp.extractor.common import InfoExtractor
        from failures import classify_error
        transport.configure(0.25, 1024, 2)
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "proxy": ""}) as downloader, self.urllib_wire(downloader, [(302, {"Location": "/final"}, b"x" * 4096)]) as calls, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
            ie = InfoExtractor()
            downloader.add_info_extractor(ie)
            with self.assertRaises(Exception) as caught:
                ie._request_webpage("https://example.com/start", "fixture")
            self.assertEqual(classify_error(caught.exception), "resource_limit")
            self.assertEqual(len(calls), 1)
            self.assertEqual(dns.call_count, 1)
            self.assertEqual(transport._budget.requests, 1)

    def test_urllib_refuses_eager_compression_even_for_streaming(self):
        from music_library.ytdlp import AudioYoutubeDL
        transport.configure(0.25, 1024, 1)
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "proxy": ""}) as downloader, self.urllib_wire(downloader, [(200, {"Content-Encoding": "gzip"}, b"not inflated")]) as calls, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS):
            with self.assertRaises(music_engine.MediaFailure) as caught:
                downloader.urlopen("https://example.com/audio")
            self.assertEqual(caught.exception.failure_class, "source_unavailable")
            self.assertEqual(len(calls), 1)

    def test_requests_budget_wrapper_survives_nested_guard_context(self):
        import io
        import requests
        class Wire(requests.adapters.HTTPAdapter):
            def send(self, request, **kwargs):
                response = requests.Response()
                response.status_code = 200
                response.url, response.request = request.url, request
                response.raw = io.BytesIO(b"x" * 4096)
                return response
        transport.configure(0.25, 1024, 1)
        previous = requests.Session.send
        with transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), transport.requests_budget(), requests.Session() as session, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
            session.mount("https://", Wire())
            wrapped = requests.Session.send
            with transport.guard.guard_context(()):
                self.assertIs(requests.Session.send, wrapped)
                with self.assertRaises(music_engine.MediaFailure):
                    session.get("https://example.com/metadata")
            self.assertIs(requests.Session.send, wrapped)
            self.assertEqual(dns.call_count, 1)
        self.assertIs(requests.Session.send, previous)

    def test_real_extractor_retry_charges_each_wire(self):
        from music_library.ytdlp import AudioYoutubeDL
        from yt_dlp.extractor.common import InfoExtractor
        from yt_dlp.utils import ExtractorError
        for maximum in (1, 2):
            transport.configure(0.25, 1024, maximum)
            with self.subTest(maximum=maximum), transport.guard.guard_context((), source_wire_admission=transport._budget.reserve), AudioYoutubeDL({"quiet": True, "proxy": "", "extractor_retries": 1}) as downloader, self.urllib_wire(downloader, [(503, {}, b"busy"), (200, {}, b"success")]) as calls, patch.object(transport.guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
                ie = InfoExtractor()
                downloader.add_info_extractor(ie)
                def retry_request():
                    for retry in ie.RetryManager():
                        try:
                            return ie._request_webpage("https://example.com/retry", "fixture")
                        except ExtractorError as error:
                            retry.error = error
                if maximum == 1:
                    with self.assertRaises(music_engine.MediaFailure):
                        retry_request()
                else:
                    with retry_request() as response:
                        self.assertEqual(response.read(), b"success")
                self.assertEqual(dns.call_count, maximum)
                self.assertEqual(len(calls), maximum)
                self.assertEqual(transport._budget.requests, 0)

    def test_wire_callback_threads_nested_contexts_and_restoration(self):
        import io
        import threading
        import urllib.request
        from urllib.response import addinfourl
        guard = transport.guard
        transport.configure(0.25, 1024, 1)
        errors, sent = [], []
        class Wire(urllib.request.BaseHandler):
            def https_open(self, request):
                sent.append(request.full_url)
                return addinfourl(io.BytesIO(b""), {}, request.full_url, 200)
        opener = urllib.request.OpenerDirector()
        opener.add_handler(Wire())
        previous_open, previous_callback = urllib.request.OpenerDirector.open, guard._SOURCE_WIRE_ADMISSION
        def request():
            try:
                opener.open("https://example.com/fragment").close()
            except Exception as error:
                errors.append(error)
        with guard.guard_context((), source_wire_admission=transport._budget.reserve), patch.object(guard, "resolve_addresses", return_value=PUBLIC_DNS) as dns:
            outer = guard._SOURCE_WIRE_ADMISSION
            with guard.guard_context(()):
                self.assertEqual(guard._SOURCE_WIRE_ADMISSION, outer)
                threads = [threading.Thread(target=request) for _ in range(8)]
                for thread in threads:
                    thread.start()
                for thread in threads:
                    thread.join()
            with guard.guard_context((), source_wire_admission=None):
                self.assertIsNone(guard._SOURCE_WIRE_ADMISSION)
            self.assertEqual(guard._SOURCE_WIRE_ADMISSION, outer)
            self.assertEqual(dns.call_count, 1)
            self.assertEqual(len(sent), 1)
            self.assertEqual(len(errors), 7)
            self.assertTrue(all(isinstance(error, music_engine.MediaFailure) for error in errors))
        self.assertIs(urllib.request.OpenerDirector.open, previous_open)
        self.assertEqual(guard._SOURCE_WIRE_ADMISSION, previous_callback)

    def test_music_mp3_has_real_id3_tags_and_192k_encoder_contract(self):
        import media_engine
        from mutagen.id3 import ID3
        args = media_engine._ffmpeg_args("ffmpeg", Path("source"), Path("out.mp3"), {"audio_codec": "mp3", "_music_audio_bitrate": 192}, "audio")
        self.assertEqual(args[args.index("-b:a") + 1], "192k")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.mp3"
            subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.1", "-c:a", "libmp3lame", "-b:a", "192k", str(path)], check=True)
            artifact = {"path": str(path), "size_bytes": path.stat().st_size, "media_type": "audio/mpeg"}
            cover = httpx.Response(200, content=b"\xff\xd8\xfffixture", request=httpx.Request("GET", "https://example.com/cover"))
            with patch.object(media_engine, "download_media", return_value=[artifact]) as download, patch.object(transport, "AsyncClient", return_value=Client(cover)):
                result = music_engine.audio({"url": "https://music.youtube.com/watch?v=abcdefghijk", "title": "Песня", "artist": "Артист", "thumbnail_url": "https://example.com/cover"}, None, SimpleNamespace(max_bytes=1024 * 1024))
                from music_library.ytdlp import AudioYoutubeDL
                self.assertIs(download.call_args.kwargs["youtube_dl_factory"], AudioYoutubeDL)
            tags = ID3(path)
            self.assertEqual(str(tags["TIT2"]), "Песня")
            self.assertEqual(str(tags["TPE1"]), "Артист")
            self.assertEqual(tags.getall("APIC")[0].data, b"\xff\xd8\xfffixture")
            self.assertEqual(result[0]["size_bytes"], path.stat().st_size)

if __name__ == "__main__":
    unittest.main()
