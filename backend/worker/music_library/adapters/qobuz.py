"""Qobuz search adapter - public autosuggest API, no auth required."""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from music_library.utils import track_score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.5
# Autosuggest endpoint extracted from qobuz.com page source (idonthavespotify reference)
_AUTOSUGGEST_URL = "https://www.qobuz.com/v4/us-en/catalog/search/autosuggest"


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        resp = await client.get(
            _AUTOSUGGEST_URL,
            params={"q": query, "limit": "5"},
            headers={"X-Requested-With": "XMLHttpRequest"},
        )
        resp.raise_for_status()
        data = resp.json()

        tracks = data.get("tracks", [])
        if not tracks:
            return None

        best_url: str | None = None
        best_score = 0.0
        for item in tracks:
            c_title = (item.get("title") or "").strip()
            c_artist = (item.get("artist") or "").strip()
            # Use album URL from autosuggest - Qobuz doesn't expose direct track URLs publicly
            url = (item.get("url") or "").strip()
            if not url:
                continue
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("Qobuz score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score, best_url = s, url

        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("Qobuz search failed: %s", exc)
        return None
