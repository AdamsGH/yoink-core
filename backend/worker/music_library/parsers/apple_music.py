"""Apple Music parser - og-tags scrape.

og:title format: "Track Title by Artist on Apple Music"
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
    r"music\.apple\.com/(?:[a-z]{2}/)?album/[^/?]+/\d+\?i=\d+"
    r"|music\.apple\.com/(?:[a-z]{2}/)?song/"
)


async def parse(url: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    # geo.music.apple.com -> music.apple.com
    clean_url = url.replace("geo.music.apple.com", "music.apple.com")

    resp = await client.get(clean_url)
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
        raise ResolverError(f"Apple Music: could not extract og:title from {clean_url}")

    # Strip " on Apple Music" suffix (localised: "on", "bei", "en", "sur", etc.)
    cleaned = re.sub(
        r"\s+(?:on|bei|en|sur|su|no|op|på|w)\s+Apple\s+Music$", "", og_title, flags=re.IGNORECASE
    ).strip()

    # Split "Title by Artist" — greedy to handle titles containing "de"/"di"
    m = re.match(r"^(.+)\s+(?:by|von|de|par|di|door|av|af|przez)\s+(.+)$", cleaned, re.IGNORECASE)
    if m:
        title = m.group(1).strip()
        artist = m.group(2).strip()
    else:
        title = cleaned
        artist = ""

    return title, artist, thumbnail
