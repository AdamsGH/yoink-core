"""Qobuz parser - og-tags scrape from www.qobuz.com.

Qobuz URLs come in two shapes:
  - www.qobuz.com/<locale>/album/<slug>/<id>  (album page, og-tags available)
  - www.qobuz.com/<locale>/track/<slug>/<id>  (redirects to album page)

og:title format: "Album Title, Artist - Qobuz"
"""
from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

TRACK_RE = re.compile(
    r"(?:www\.)?qobuz\.com/[a-z]{2}(?:-[a-z]{2})?/(?:album|track)/[^/\s,]+/[A-Za-z0-9]+"
)


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
        raise ResolverError(f"Qobuz: could not extract og:title from {url}")

    # og:title format: "Album Title, Artist - Qobuz"
    # Strip " - Qobuz" suffix
    cleaned = re.sub(r"\s*-\s*Qobuz\s*$", "", og_title, flags=re.IGNORECASE).strip()

    # Split on last ", " to get artist
    if ", " in cleaned:
        title, _, artist = cleaned.rpartition(", ")
    else:
        title = cleaned
        artist = ""

    return title.strip(), artist.strip(), thumbnail
