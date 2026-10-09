"""Apple Music search adapter - iTunes Search API."""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from music_library.utils import track_score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_ITUNES_SEARCH_URL = "https://itunes.apple.com/search"
_MIN_SCORE = 0.4


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        resp = await client.get(
            _ITUNES_SEARCH_URL,
            params={"term": query, "entity": "song", "limit": "5"},
        )
        resp.raise_for_status()
        data = resp.json()
        results = data.get("results", [])
        if not results:
            return None

        best_url: str | None = None
        best_score = 0.0
        for item in results:
            if item.get("kind") != "song":
                continue
            c_title = item.get("trackName", "")
            c_artist = item.get("artistName", "")
            track_url = item.get("trackViewUrl", "")
            if not track_url:
                continue
            s = track_score(c_artist, c_title, artist, title) if title else 0.0
            logger.debug("Apple Music score=%.2f title=%r artist=%r", s, c_title, c_artist)
            if s > best_score:
                best_score, best_url = s, track_url

        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("Apple Music search failed: %s", exc)
        return None
