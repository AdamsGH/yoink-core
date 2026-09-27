"""Classified, source-redacted failures shared by downloader adapters."""
from __future__ import annotations

import re


def classify_error(exc: BaseException) -> str:
    chain: list[BaseException] = []
    current: BaseException | None = exc
    while current is not None and current not in chain:
        chain.append(current)
        current = current.__cause__ or current.__context__
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
