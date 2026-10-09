"""Spotify parser - extracts track metadata.

Two modes (tried in order):
  1. Official Web API (Client Credentials) - if MUSIC_SPOTIFY_CLIENT_ID + SECRET are set.
     Returns full title + artist + thumbnail.
  2. oEmbed fallback - no auth needed, but only gives title + thumbnail.
     Artist is filled in later by the Deezer adapter search.

Proxy: if MUSIC_SPOTIFY_PROXY is set (e.g. http://user:pass@host:port),
all Spotify requests are routed through it.
"""
from __future__ import annotations

import logging
import re
import time
from contextlib import asynccontextmanager

from music_library import transport as httpx

from music_library.types import ResolverError

logger = logging.getLogger(__name__)

TRACK_RE = re.compile(r"open\.spotify\.com/(?:intl-[a-z]{2}/)?track/([A-Za-z0-9]+)")
_TOKEN_CACHE: dict[str, object] = {}


@asynccontextmanager
async def _nullctx(client: httpx.AsyncClient):
    """No-op async context manager - yields the client without closing it."""
    yield client


def _make_client(proxy: str | None, timeout: float) -> httpx.AsyncClient:
    kwargs: dict = {"timeout": timeout, "follow_redirects": True}
    if proxy:
        kwargs["proxy"] = proxy
    return httpx.AsyncClient(**kwargs)


async def _get_access_token(client_id: str, client_secret: str, proxy: str | None, timeout: float) -> str:
    """Spotify Client Credentials flow - no user auth needed."""
    cache = _TOKEN_CACHE.get("spotify")
    if cache and cache["expires_at"] > time.monotonic() + 60:  # type: ignore[index]
        return cache["token"]  # type: ignore[index]

    async with _make_client(proxy, timeout) as c:
        resp = await c.post(
            "https://accounts.spotify.com/api/token",
            data={"grant_type": "client_credentials"},
            auth=(client_id, client_secret),
        )
        resp.raise_for_status()
        data = resp.json()

    token = data["access_token"]
    expires_in = data.get("expires_in", 3600)
    _TOKEN_CACHE["spotify"] = {"token": token, "expires_at": time.monotonic() + expires_in}
    logger.debug("Spotify: got new access token (expires in %ds)", expires_in)
    return token


async def parse(
    url: str,
    http_client: httpx.AsyncClient,
    *,
    client_id: str | None = None,
    client_secret: str | None = None,
    proxy: str | None = None,
) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url).

    artist may be empty string if falling back to oEmbed.
    """
    m = TRACK_RE.search(url)
    if not m:
        raise ResolverError(f"Cannot extract Spotify track ID from {url}")
    track_id = m.group(1)

    if client_id and client_secret:
        try:
            return await _parse_via_api(track_id, client_id, client_secret, proxy, http_client.timeout.read or 8)
        except Exception as exc:
            logger.warning("Spotify API failed, falling back to embed scrape: %s", exc)

    # Embed scrape - works without API key, requires proxy for some regions
    embed_client = _make_client(proxy, http_client.timeout.read or 8) if proxy else http_client
    try:
        async with embed_client if proxy else _nullctx(embed_client) as c:
            return await _parse_via_embed(track_id, c)
    except Exception as exc:
        logger.warning("Spotify embed scrape failed, falling back to oEmbed: %s", exc)

    # Last resort: oEmbed - title only, no artist
    resp = await http_client.get(
        f"https://open.spotify.com/oembed?url=https://open.spotify.com/track/{track_id}"
    )
    resp.raise_for_status()
    data = resp.json()
    title = data.get("title") or ""
    if not title:
        raise ResolverError("Spotify oEmbed returned empty title")
    return title, "", data.get("thumbnail_url")


async def _parse_via_api(
    track_id: str,
    client_id: str,
    client_secret: str,
    proxy: str | None,
    timeout: float,
) -> tuple[str, str, str | None]:
    token = await _get_access_token(client_id, client_secret, proxy, timeout)
    async with _make_client(proxy, timeout) as c:
        resp = await c.get(
            f"https://api.spotify.com/v1/tracks/{track_id}",
            headers={"Authorization": f"Bearer {token}"},
        )
        resp.raise_for_status()
        data = resp.json()

    title = data.get("name") or ""
    artist = ", ".join(a["name"] for a in data.get("artists", []))
    images = data.get("album", {}).get("images", [])
    thumbnail = images[0]["url"] if images else None
    if not title:
        raise ResolverError("Spotify API returned empty title")
    return title, artist, thumbnail


async def _parse_via_embed(track_id: str, client: httpx.AsyncClient) -> tuple[str, str, str | None]:
    """Scrape the embed page - works without auth, returns full metadata.

    The embed page at open.spotify.com/embed/track/{id} contains a __NEXT_DATA__
    JSON blob with title, artists list, and album art URLs. Requires the Spotify
    proxy since the embed is geo-restricted from some IPs.
    """
    resp = await client.get(
        f"https://open.spotify.com/embed/track/{track_id}",
        headers={
            "User-Agent": (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
                "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
            ),
            "Accept-Language": "en-US,en;q=0.9",
        },
    )
    resp.raise_for_status()

    import json as _json
    m = re.search(r'<script id="__NEXT_DATA__"[^>]*>(.*?)</script>', resp.text, re.DOTALL)
    if not m:
        raise ResolverError("Spotify embed: __NEXT_DATA__ not found")

    data = _json.loads(m.group(1))
    entity = data["props"]["pageProps"]["state"]["data"]["entity"]

    title = entity.get("name") or entity.get("title") or ""
    if not title:
        raise ResolverError("Spotify embed: empty title")

    artists = entity.get("artists") or []
    artist = ", ".join(a["name"] for a in artists if a.get("name"))

    images = entity.get("visualIdentity", {}).get("image") or []
    # pick largest image (last in list has highest maxWidth)
    thumbnail = images[-1].get("url") if images else None

    return title, artist, thumbnail
