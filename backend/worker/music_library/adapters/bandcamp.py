"""Bandcamp search adapter - public autocomplete API, no auth required."""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from music_library.utils import track_score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.5
_API_URL = "https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic"


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        resp = await client.post(
            _API_URL,
            json={"search_text": query, "search_filter": "t", "full_page": False, "fan_id": None},
            headers={"Accept": "application/json", "Content-Type": "application/json"},
        )
        resp.raise_for_status()
        data = resp.json()

        results = data.get("auto", {}).get("results", [])
        if not results:
            return None

        best_url: str | None = None
        best_score = 0.0
        for item in results[:5]:
            if item.get("type") != "t":
                continue
            c_title = (item.get("name") or "").strip()
            c_artist = (item.get("band_name") or "").strip()
            url = str(item.get("item_url_path") or "").strip()
            if not url:
                continue
            if not url.startswith("http"):
                url = f"https://bandcamp.com{url}"
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("Bandcamp score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score, best_url = s, url

        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("Bandcamp search failed: %s", exc)
        return None
