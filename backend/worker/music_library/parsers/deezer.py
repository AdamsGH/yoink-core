"""Deezer parser - uses public Deezer API, no auth needed."""
from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING
from urllib.parse import unquote, urljoin

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

TRACK_RE = re.compile(r"deezer\.com/(?:[a-z]{2}/)?track/(\d+)", re.IGNORECASE)
URL_RE = re.compile(
    r"(?:[\w-]+\.)*deezer\.com(?:/[a-z]{2})?/(?:album|track)/[^\s.,]+"
    r"|(?:link\.deezer\.com|deezer\.page\.link)/[^\s.,]+",
    re.IGNORECASE,
)


async def _extract_track_id(url: str, client: httpx.AsyncClient) -> str | None:
    """Extract track ID directly or by following redirects for short links."""
    m = TRACK_RE.search(url)
    if m:
        return m.group(1)

    # Inspect each redirect before requesting its destination: the track page
    # itself may be unavailable even though its ID is already in Location.
    for method in ("HEAD", "GET"):
        current = url
        try:
            for _ in range(client.max_redirects + 1):
                async with client.stream(method, current, follow_redirects=False) as resp:
                    effective = str(resp.url)
                    m = TRACK_RE.search(effective) or TRACK_RE.search(unquote(effective))
                    if m:
                        return m.group(1)
                    loc = resp.headers.get("location")
                    if not resp.has_redirect_location or not loc:
                        break
                    current = urljoin(effective, loc)
                    m = TRACK_RE.search(current) or TRACK_RE.search(unquote(current))
                    if m:
                        return m.group(1)
        except Exception as exc:
            logger.debug("Deezer %s redirect resolution failed for %s: %s", method, url, exc)

    return None


async def parse(url: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    track_id = await _extract_track_id(url, client)
    if not track_id:
        raise ResolverError(f"Cannot extract Deezer track ID from {url}")

    resp = await client.get(f"https://api.deezer.com/track/{track_id}")
    resp.raise_for_status()
    data = resp.json()

    if "error" in data:
        raise ResolverError(f"Deezer API error: {data['error']}")

    title = data.get("title") or ""
    artist = data.get("artist", {}).get("name") or ""
    thumbnail = data.get("album", {}).get("cover_medium")
    if not title:
        raise ResolverError("Deezer API returned empty title")
    return title, artist, thumbnail
