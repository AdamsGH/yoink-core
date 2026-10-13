"""Anonymous SDK caption discovery through the existing bounded transport."""
from io import BytesIO
from urllib.parse import parse_qs, urlsplit
from urllib.request import Request
from urllib.response import addinfourl

import yt_dlp
from yt_dlp.networking import Response
import network_guard as guard

from content_transport import ContentError


class QuietLogger:
    def debug(self, message):
        pass

    warning = debug
    error = debug


class CaptionDL(yt_dlp.YoutubeDL):
    def __init__(self, transport):
        self.transport = transport
        super().__init__({
            'skip_download': True, 'noplaylist': True, 'quiet': True,
            'noprogress': True, 'logger': QuietLogger(), 'cachedir': False,
            'nocheckcertificate': False, 'retries': 0, 'extractor_retries': 0,
            'ignore_no_formats_error': True, 'impersonate': None,
            'remote_components': [], 'js_runtimes': {},
            'extractor_args': {'youtube': {
                'fetch_pot': ['never'], 'skip': ['hls', 'dash', 'translated_subs'],
            }},
        })

    def urlopen(self, request):
        headers = {**self.params['http_headers'], **request.headers}
        headers['Accept-Encoding'] = 'identity'
        cookie_request = Request(request.url, data=request.data, headers=headers, method=request.method)
        self.cookiejar.add_cookie_header(cookie_request)
        body, url, headers, status = self.transport.request(
            request.url, request.method, request.data, dict(cookie_request.header_items()))
        self.cookiejar.extract_cookies(addinfourl(BytesIO(body), headers, url), Request(url))
        return Response(BytesIO(body), url, headers, status)


def caption_tracks(transport, video_id):
    before = guard.guard_failure_count()
    try:
        with CaptionDL(transport) as client:
            info = client.extract_info('https://www.youtube.com/watch?v=' + video_id, download=False)
    finally:
        # The SDK may wrap relay-thread denials in DownloadError.
        if guard.guard_failure_count() != before:
            raise guard.NetworkPolicyError('network destination is not permitted')
    if not isinstance(info, dict) or info.get('id') != video_id:
        raise ContentError('content_invalid_response')
    tracks = []
    seen = set()
    for kind in ('subtitles', 'automatic_captions'):
        captions = info.get(kind) or {}
        if not isinstance(captions, dict):
            raise ContentError('content_invalid_response')
        for language, candidates in captions.items():
            if not isinstance(language, str) or not isinstance(candidates, list):
                raise ContentError('content_invalid_response')
            for candidate in candidates:
                if not isinstance(candidate, dict):
                    raise ContentError('content_invalid_response')
                url = candidate.get('url')
                if candidate.get('ext') != 'json3' or not isinstance(url, str) or url in seen:
                    continue
                # Prefer original speech, not automatically translated captions.
                if 'tlang' in parse_qs(urlsplit(url).query):
                    continue
                seen.add(url)
                tracks.append({'baseUrl': url, 'languageCode': language.removesuffix('-orig'),
                               'kind': 'asr' if kind == 'automatic_captions' else ''})
    return tracks
