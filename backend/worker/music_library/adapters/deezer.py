"""Deezer search adapter - public API, no auth."""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from music_library.utils import track_score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.6


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        resp = await client.get(
            "https://api.deezer.com/search/track",
            params={"q": query, "limit": "5"},
        )
        data = resp.json()
        if not data.get("data"):
            return None
        best_url, best_score = None, 0.0
        for item in data["data"]:
            c_artist = item.get("artist", {}).get("name", "")
            c_title = item.get("title", "")
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("Deezer score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score, best_url = s, item.get("link")
        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("Deezer search failed: %s", exc)
        return None
