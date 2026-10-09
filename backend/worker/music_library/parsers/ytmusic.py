"""YouTube Music parser - uses ytmusicapi, no API key needed."""
from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

TRACK_RE = re.compile(r"music\.youtube\.com/watch\?.*v=([A-Za-z0-9_-]+)")


async def parse(url: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    m = TRACK_RE.search(url)
    if not m:
        raise ResolverError(f"Cannot extract YouTube Music video ID from {url}")
    video_id = m.group(1)

    try:
        from ytmusicapi import YTMusic
        ytm = YTMusic()
        info = ytm.get_song(video_id)
    except ImportError as exc:
        raise ResolverError("ytmusicapi not installed") from exc
    except Exception as exc:
        raise ResolverError(f"YTMusic parse failed: {exc}") from exc

    details = info.get("videoDetails", {})
    title = details.get("title") or ""
    artist = details.get("author") or ""
    thumbnails = details.get("thumbnail", {}).get("thumbnails", [])
    thumbnail = thumbnails[-1]["url"] if thumbnails else None
    if not title:
        raise ResolverError("YTMusic: empty title")
    return title, artist, thumbnail
