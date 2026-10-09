"""Music downloader admission with bounded extraction and streaming media."""
import yt_dlp
from contextvars import ContextVar
from functools import wraps
import urllib.request
from yt_dlp.networking.common import Request, Response
from yt_dlp.compat import urllib_req_to_req
from yt_dlp.networking.exceptions import HTTPError, NoSupportingHandlers, TransportError
from yt_dlp.networking._urllib import UrllibRH
from music_library import transport
from failures import MediaFailure

_extraction_request = ContextVar("music_extraction_request", default=False)
_bounded_wire = ContextVar("music_bounded_wire", default=False)

def _request(req):
    if isinstance(req, str):
        return Request(req)
    if isinstance(req, urllib.request.Request):
        return urllib_req_to_req(req)
    return req

def _bounded(response):
    class BoundedResponse(Response):
        remaining = transport._budget.maximum
        def read(self, amt=None):
            try:
                data = super().read(min(self.remaining + 1, amt if amt is not None and amt >= 0 else self.remaining + 1))
            except TransportError as exc:
                if isinstance(exc.cause, MediaFailure):
                    self.close()
                    raise exc.cause from exc
                raise
            self.remaining -= len(data)
            if self.remaining < 0:
                self.close()
                raise MediaFailure("resource_limit")
            return data
    return BoundedResponse(response, response.url, response.headers, response.status, response.reason, response.extensions)

def _open(req, open_request, bounded):
    req = _request(req)
    if transport.guard._SOURCE_WIRE_ADMISSION != transport._budget.reserve:
        raise MediaFailure("resource_limit")
    # yt-dlp otherwise strips credentials/control characters before the wire guard.
    transport.guard._parse_http_url(req.url, "invalid source")
    req.extensions["timeout"] = min(req.extensions.get("timeout") or transport._budget.timeout, transport._budget.timeout)
    token = _bounded_wire.set(bounded)
    try:
        response = open_request(req)
    except HTTPError as exc:
        if bounded:
            exc.response = _bounded(exc.response)
        raise
    except NoSupportingHandlers as exc:
        for error in exc.unexpected_errors:
            if isinstance(error, MediaFailure):
                raise error from exc
        raise
    finally:
        _bounded_wire.reset(token)
    return _bounded(response) if bounded else response

class MusicUrllibRH(UrllibRH):
    RH_KEY = "Urllib"
    _SUPPORTED_URL_SCHEMES = ("http", "https")
    def __init__(self, *, enable_file_urls=False, **kwargs):
        super().__init__(enable_file_urls=False, **kwargs)
    def _create_instance(self, *args, **kwargs):
        opener = super()._create_instance(*args, **kwargs)
        class ResponsePolicy(urllib.request.BaseHandler):
            handler_order = 0
            def http_request(self, request):
                request.add_header("Accept-Encoding", "identity")
                return request
            https_request = http_request
            def http_response(self, request, response):
                # yt-dlp's urllib decoder eagerly inflates compressed bodies.
                # Keep this covered handler streaming and fail closed if the
                # server ignores our explicit identity encoding request.
                if response.headers.get("Content-Encoding", "identity").lower() != "identity":
                    response.close()
                    raise MediaFailure("source_unavailable")
                if _bounded_wire.get():
                    body = _bounded(Response(response, response.url, response.headers, response.code, response.msg))
                    result = urllib.request.addinfourl(body, response.headers, response.url, response.code)
                    result.msg = response.msg
                    return result
                return response
            https_response = http_response
        opener.add_handler(ResponsePolicy())
        return opener

class WireYoutubeDL(yt_dlp.YoutubeDL):
    def build_request_director(self, handlers, preferences=None):
        # Urllib redirects re-enter the guarded opener. Other handlers can retry
        # below the guard or use independent networking and are not admitted.
        return super().build_request_director(
            [MusicUrllibRH] if any(handler.RH_KEY == "Urllib" for handler in handlers) else [], preferences)

class YoutubeDL(WireYoutubeDL):
    def __init__(self, options):
        super().__init__({**options, "ignoreconfig": True, "plugin_dirs": [], "cachedir": False, "cookiefile": None, "cookiesfrombrowser": None, "retries": 0, "extractor_retries": 0, "socket_timeout": transport._budget.timeout, "proxy": options.get("proxy") or "", "playlistend": 10})
    def urlopen(self, req):
        return _open(req, super().urlopen, bounded=True)

class AudioYoutubeDL(WireYoutubeDL):
    """Bound extractor bodies, not audio streams governed by media quotas."""
    def __init__(self, options):
        super().__init__({**options, "socket_timeout": min(options.get("socket_timeout") or transport._budget.timeout, transport._budget.timeout)})
    def add_info_extractor(self, ie):
        super().add_info_extractor(ie)
        if isinstance(ie, type) or getattr(ie, "_music_request_wrapped", False):
            return
        original = ie._request_webpage
        @wraps(original)
        def request_webpage(*args, **kwargs):
            token = _extraction_request.set(True)
            try:
                return original(*args, **kwargs)
            finally:
                _extraction_request.reset(token)
        ie._request_webpage = request_webpage
        ie._music_request_wrapped = True
    def urlopen(self, req):
        return _open(req, super().urlopen, bounded=_extraction_request.get())
