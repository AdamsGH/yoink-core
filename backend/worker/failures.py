"""Classified, source-redacted failures shared by downloader adapters."""
from __future__ import annotations

import re


class MediaFailure(Exception):
    """A classified worker failure with no source-derived diagnostic."""

    def __init__(self, failure_class: str, *, provider_failure: bool = False):
        if failure_class not in {
            "unsupported_source", "source_unavailable", "rate_limited",
            "invalid_media", "processing_failed", "resource_limit",
        }:
            failure_class = "invalid_media"
        self.failure_class = failure_class
        # Categories are not provider authority; local failures deny fallback by default.
        self.fallback_forbidden = not provider_failure
        super().__init__(failure_class)


def classify_error(exc: BaseException) -> str:
    chain = exception_chain(exc)
    allowed = {"unsupported_source", "source_unavailable", "rate_limited", "invalid_media", "processing_failed", "resource_limit"}
    for item in chain:
        classified = getattr(item, "failure_class", None)
        if classified in allowed:
            return classified
    messages = " ".join(str(item).lower() for item in chain)
    if any("limit" in type(item).__name__.lower() for item in chain) or "budget" in messages or "too large" in messages or "resource limit" in messages:
        return "resource_limit"
    if "unsupported" in messages or "not a video" in messages or "no suitable" in messages:
        return "unsupported_source"
    statuses = {getattr(item, "code", None) for item in chain}
    statuses.update(getattr(item, "status", None) for item in chain)
    statuses.update(int(match) for match in re.findall(r"http error\s+(\d{3})\b", messages))
    if 429 in statuses or "rate limit" in messages:
        return "rate_limited"
    if statuses.intersection({408, 500, 502, 503, 504}):
        return "source_unavailable"
    # HTTPError is also an OSError. Known permanent responses must not be retried.
    if any(isinstance(status, int) and 400 <= status < 500 for status in statuses):
        return "invalid_media"
    transient_words = ("timed out", "timeout", "connection reset", "connection aborted", "remote end closed connection")
    if any(isinstance(item, (OSError, TimeoutError)) for item in chain) or any(word in messages for word in transient_words) or "network destination" in messages:
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
