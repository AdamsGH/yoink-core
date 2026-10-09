"""Tidal parser - og-tags scrape from listen.tidal.com.

og:title format: "Artist - Track Title"
og:type: music.song / music.album / etc.
"""
from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

# listen.tidal.com/track/12345 or tidal.com/browse/track/12345
TRACK_RE = re.compile(
    r"(?:listen\.)?tidal\.com(?:/browse)?/(?:track|album)/\d+(?:/track/\d+)?"
)


async def parse(url: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    # Normalize to listen.tidal.com for og-tag serving
    clean = re.sub(r"tidal\.com/browse/", "listen.tidal.com/", url)
    clean = re.sub(r"^https?://(?:www\.)?tidal\.com/", "https://listen.tidal.com/", clean)

    resp = await client.get(clean)
    resp.raise_for_status()
    html = resp.text

    def og(prop: str) -> str | None:
        m = re.search(
            rf'<meta[^>]+property=["\']og:{prop}["\'][^>]+content=["\']([^"\']+)["\']',
            html, re.IGNORECASE,
        ) or re.search(
            rf'<meta[^>]+content=["\']([^"\']+)["\'][^>]+property=["\']og:{prop}["\']',
            html, re.IGNORECASE,
        )
        return m.group(1) if m else None

    og_title = og("title") or ""
    thumbnail = og("image")

    if not og_title:
        raise ResolverError(f"Tidal: could not extract og:title from {clean}")

    # og:title format: "Artist - Track Title"
    if " - " in og_title:
        artist, _, title = og_title.partition(" - ")
    else:
        title = og_title
        artist = ""

    return title.strip(), artist.strip(), thumbnail
