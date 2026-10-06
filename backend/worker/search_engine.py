"""Pinned YouTube search, flat metadata only; no cookies, files or media."""
from __future__ import annotations

import math
import re
import unicodedata
from urllib.parse import urlsplit, parse_qs

from failures import MediaFailure
from network_guard import guard_failure_count

MAX_QUERY_BYTES = 1024
MAX_QUERY_CHARS = 256
MAX_RESULTS = 8
MAX_SEARCH_FRAME_BYTES = 32 * 1024
MAX_RESPONSE_BYTES = 2 * 1024 * 1024
MAX_REQUESTS = 16
_ID = re.compile(r"^[A-Za-z0-9_-]{11}$")


def unsafe_char(char):
    return (unicodedata.category(char) == "Cc" or ord(char) == 0x061c
            or 0x200b <= ord(char) <= 0x200f or 0x2028 <= ord(char) <= 0x202e
            or 0x2060 <= ord(char) <= 0x206f or ord(char) == 0xfeff)


def validate_query(query):
    if (not isinstance(query, str) or not 2 <= len(query) <= MAX_QUERY_CHARS
            or len(query.encode("utf-8")) > MAX_QUERY_BYTES or query.strip() != query
            or any(unsafe_char(c) for c in query)):
        raise ValueError("search query must be 2 through 256 clean characters (at most 1024 bytes)")


def clean(value, maximum):
    if not isinstance(value, str):
        return ""
    return "".join(" " if unsafe_char(c) else c for c in value[:maximum]).strip()


def normalize_entries(entries):
    result, seen = [], set()
    # Materialized extractor output is bounded by ytsearch8 and HTTP response budgets.
    if not isinstance(entries, list) or len(entries) > 64:
        raise MediaFailure("resource_limit")
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        ident = entry.get("id")
        raw = entry.get("url") or entry.get("webpage_url")
        if not isinstance(ident, str) or not _ID.fullmatch(ident) or not isinstance(raw, str) or len(raw) > 256:
            continue
        try:
            url = urlsplit(raw)
            if (url.scheme != "https" or url.hostname not in ("www.youtube.com", "youtube.com")
                    or url.username is not None or url.password is not None or url.port is not None
                    or url.path != "/watch" or url.fragment or parse_qs(url.query) != {"v": [ident]}):
                continue
        except ValueError:
            continue
        if ident in seen:
            continue
        seen.add(ident)
        duration = entry.get("duration")
        if type(duration) not in (int, float) or not math.isfinite(duration) or not 0 <= duration <= 31_536_000:
            duration = None
        else:
            duration = int(duration)
        views = entry.get("view_count")
        if type(views) is not int or not 0 <= views <= 1_000_000_000_000_000:
            views = None
        result.append({"url": f"https://www.youtube.com/watch?v={ident}",
                       "title": clean(entry.get("title"), 120),
                       "channel": clean(entry.get("channel") or entry.get("uploader"), 100),
                       "duration_seconds": duration, "view_count": views})
        if len(result) == MAX_RESULTS:
            break
    return result


def search(request, settings):
    import yt_dlp
    from yt_dlp.networking.common import Response

    query = request["task"]["query"]
    validate_query(query)
    network = request.get("network") or {}
    initial_failures = guard_failure_count()

    class BoundedResponse(Response):
        def __init__(self, response):
            super().__init__(response, response.url, response.headers, response.status,
                             response.reason, response.extensions)
            self.remaining = MAX_RESPONSE_BYTES
        def read(self, amt=None):
            data = super().read(min(self.remaining + 1, amt if amt is not None and amt >= 0 else self.remaining + 1))
            self.remaining -= len(data)
            if self.remaining < 0:
                self.close()
                raise MediaFailure("resource_limit")
            return data

    class SearchDownloader(yt_dlp.YoutubeDL):
        requests = 0
        def urlopen(self, req):
            self.requests += 1
            if self.requests > MAX_REQUESTS:
                raise MediaFailure("resource_limit")
            response = super().urlopen(req)
            declared = response.headers.get("Content-Length")
            if declared and declared.isdigit() and int(declared) > MAX_RESPONSE_BYTES:
                response.close()
                raise MediaFailure("resource_limit")
            return BoundedResponse(response)

    class SilentLogger:
        def debug(self, *_args): pass
        def warning(self, *_args): pass
        def error(self, *_args): pass
    opts = {"quiet": True, "no_warnings": True, "logger": SilentLogger(), "skip_download": True,
            "extract_flat": True, "lazy_playlist": False, "playlistend": MAX_RESULTS,
            "ignoreconfig": True, "plugin_dirs": [], "cachedir": False,
            "cookiefile": None, "cookiesfrombrowser": None,
            "proxy": network.get("proxy_url") or "", "socket_timeout": settings.socket_timeout_secs,
            "retries": 0, "extractor_retries": 0, "fragment_retries": 0,
            "postprocessors": [], "writesubtitles": False, "writeautomaticsub": False,
            "writethumbnail": False, "writeinfojson": False, "external_downloader": None,
            "impersonate": None, "compat_opts": set(),
            "extractor_args": {"youtubetab": {"skip": ["webpage"]}}}
    with SearchDownloader(opts) as downloader:
        info = downloader.extract_info(f"ytsearch{MAX_RESULTS}:{query}", download=False)
    # Never mask a networking denial as a successful empty search.
    if guard_failure_count() != initial_failures:
        raise MediaFailure("source_unavailable")
    if not isinstance(info, dict):
        raise MediaFailure("invalid_media")
    return normalize_entries(info.get("entries") or [])
