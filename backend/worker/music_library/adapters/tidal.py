"""Tidal search adapter - official API with OAuth2 client_credentials."""
from __future__ import annotations

import asyncio
import logging
import time
from typing import TYPE_CHECKING

from music_library.utils import score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.6

_token_cache: tuple[str, float] = ("", 0.0)
_token_lock = asyncio.Lock()


async def get_access_token(client: httpx.AsyncClient, client_id: str, client_secret: str) -> str:
    global _token_cache
    token, expires_at = _token_cache
    if token and time.monotonic() < expires_at - 60:
        return token

    async with _token_lock:
        token, expires_at = _token_cache
        if token and time.monotonic() < expires_at - 60:
            return token

        resp = await client.post(
            "https://auth.tidal.com/v1/oauth2/token",
            data={"grant_type": "client_credentials"},
            auth=(client_id, client_secret),
            headers={"Content-Type": "application/x-www-form-urlencoded"},
        )
        resp.raise_for_status()
        data = resp.json()
        token = data["access_token"]
        expires_at = time.monotonic() + data.get("expires_in", 86400)
        _token_cache = (token, expires_at)
        logger.debug("Tidal: fetched new access token (expires_in=%s)", data.get("expires_in"))
        return token


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
    client_id: str = "",
    client_secret: str = "",
) -> str | None:
    if not client_id or not client_secret:
        return None
    try:
        token = await get_access_token(client, client_id, client_secret)

        import urllib.parse
        encoded_query = urllib.parse.quote(query, safe="")
        resp = await client.get(
            f"https://openapi.tidal.com/v2/searchResults/{encoded_query}/relationships/tracks",
            params={"countryCode": "US", "include": "tracks"},
            headers={"Authorization": f"Bearer {token}"},
        )
        resp.raise_for_status()
        data = resp.json()

        # data[] has id+type refs, included[] has attributes
        refs = data.get("data", [])
        included = data.get("included", [])
        if not refs or not included:
            return None

        # included[] and data[] are positionally paired (same as idonthavespotify)
        # Artist is not returned in attributes - match full query against title
        full_query = f"{artist} {title}".strip() if artist else title

        best_url: str | None = None
        best_score = 0.0
        for item, ref in zip(included[:5], refs[:5], strict=False):
            attrs = item.get("attributes", {})
            c_title = (attrs.get("title") or "").strip()
            track_id = ref.get("id", "")
            ext_links = attrs.get("externalLinks", [])
            url = next(
                (link["href"] for link in ext_links if link.get("meta", {}).get("type") == "TIDAL_SHARING"),
                f"https://tidal.com/browse/track/{track_id}",
            )
            s = score(c_title, full_query)
            logger.debug("Tidal score=%.2f title=%r", s, c_title)
            if s > best_score:
                best_score, best_url = s, url

        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("Tidal search failed: %s", exc)
        return None
