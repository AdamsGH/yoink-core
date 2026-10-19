"""Classified, source-redacted failures shared by downloader adapters."""
from __future__ import annotations

import re

_CLASSES = {
    "unsupported_source", "source_unavailable", "rate_limited", "invalid_media",
    "processing_failed", "resource_limit", "cookie_required", "geo_blocked",
    "age_restricted", "private_content", "live_stream", "no_media", "media_timeout",
    "network_denied", "content_unavailable", "source_blocked",
}


class MediaFailure(Exception):
    """A classified worker failure with no source-derived diagnostic."""

    def __init__(self, failure_class: str, *, provider_failure: bool = False):
        if failure_class not in _CLASSES:
            failure_class = "invalid_media"
        self.failure_class = failure_class
        # Categories are not provider authority; local failures deny fallback by default.
        self.fallback_forbidden = not provider_failure
        super().__init__(failure_class)


def classify_error(exc: BaseException) -> str:
    chain = exception_chain(exc)
    for item in chain:
        classified = getattr(item, "failure_class", None)
        if classified in _CLASSES:
            return classified
    messages = " ".join(str(item).lower() for item in chain)
    if any("limit" in type(item).__name__.lower() for item in chain) or "budget" in messages or "too large" in messages or "resource limit" in messages:
        return "resource_limit"
    if "network destination resolution failed" in messages:
        return "source_unavailable"
    if "network destination" in messages or "source address is not permitted" in messages:
        return "network_denied"
    for category, hints in (
        ("geo_blocked", ("not available in your country", "geo-restricted", "geo restricted", "region locked", "only available in", "not available outside")),
        ("age_restricted", ("confirm your age", "age-restricted", "age restricted")),
        ("private_content", ("private video", "private account", "private content", "this video is private")),
        ("live_stream", ("is a live stream", "is live", "live event", "post-live")),
        ("cookie_required", ("login required", "log in", "sign in", "cookies are required", "use --cookies", "authentication required")),
        ("no_media", ("no video could be found", "no video formats found", "no media found", "there's no video", "unable to extract media")),
        ("media_timeout", ("timed out", "timeout", "ffmpeg exited with code 146")),
    ):
        if any(hint in messages for hint in hints):
            return category
    if "unsupported" in messages or "not a video" in messages or "no suitable" in messages:
        return "unsupported_source"
    statuses = {getattr(item, "code", None) for item in chain}
    statuses.update(getattr(item, "status", None) for item in chain)
    statuses.update(int(match) for match in re.findall(r"http error\s+(\d{3})\b", messages))
    if 429 in statuses or "rate limit" in messages:
        return "rate_limited"
    if statuses.intersection({404, 410}):
        return "content_unavailable"
    if 401 in statuses:
        return "cookie_required"
    if 403 in statuses:
        return "source_blocked"
    if statuses.intersection({408, 500, 502, 503, 504}):
        return "source_unavailable"
    # HTTPError is also an OSError. Known permanent responses must not be retried.
    if any(isinstance(status, int) and 400 <= status < 500 for status in statuses):
        return "invalid_media"
    transient_words = ("timed out", "timeout", "connection reset", "connection aborted", "remote end closed connection")
    if any(isinstance(item, TimeoutError) for item in chain):
        return "media_timeout"
    if any(isinstance(item, OSError) for item in chain) or any(word in messages for word in transient_words):
        return "source_unavailable"
    if "ffprobe" in messages or "processing" in messages:
        return "processing_failed"
    return "invalid_media"


def exception_chain(exc: BaseException) -> list[BaseException]:
    """Bounded traversal of Python and pinned downloader exception links.

    yt-dlp stores originals in exc_info/cause; urllib also uses reason. Preserve
    structural policy failures even when a library omits Python cause/context links.
    """
    chain, pending, seen = [], [exc], set()
    while pending and len(chain) < 64:
        item = pending.pop()
        if not isinstance(item, BaseException) or id(item) in seen:
            continue
        seen.add(id(item))
        chain.append(item)
        pending.extend((item.__cause__, item.__context__, getattr(item, "cause", None), getattr(item, "reason", None),
                        getattr(item, "source_error", None), getattr(item, "provider_cause", None)))
        info = getattr(item, "exc_info", None)
        if isinstance(info, tuple) and len(info) == 3:
            pending.append(info[1])
    return chain
