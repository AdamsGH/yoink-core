"""Bandcamp parser - og-tags scrape.

og:title format: "Track Title, by Artist"
"""
from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

# artist.bandcamp.com/track/slug or artist.bandcamp.com/album/slug
TRACK_RE = re.compile(r"[\w-]+\.bandcamp\.com/(?:track|album)/[\w-]+")


async def parse(url: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    resp = await client.get(url)
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
        raise ResolverError(f"Bandcamp: could not extract og:title from {url}")

    # og:title format: "Track Title, by Artist"
    m = re.match(r"^(.+),\s*by\s+(.+)$", og_title, re.IGNORECASE)
    if m:
        title = m.group(1).strip()
        artist = m.group(2).strip()
    else:
        title = og_title
        artist = ""

    return title, artist, thumbnail
