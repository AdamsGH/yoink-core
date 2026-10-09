"""Spotify search adapter.

Primary: Spotify Web API search (if client_id/secret provided) - exact, scored.
Fallback: DuckDuckGo HTML search - takes top-5 results, scores via oEmbed.
"""
from __future__ import annotations

import logging
import re

from music_library import transport as httpx

from music_library.utils import track_score

logger = logging.getLogger(__name__)

_DDG_URL = "https://html.duckduckgo.com/html/"
_TRACK_URL_RE = re.compile(r"https://open\.spotify\.com/track/[A-Za-z0-9]+")
_MIN_SCORE = 0.6


async def search(
    query: str,
    client: httpx.AsyncClient,
    proxy: str | None = None,
    title: str = "",
    artist: str = "",
    client_id: str | None = None,
    client_secret: str | None = None,
) -> str | None:
    if client_id and client_secret:
        result = await _search_via_api(
            query, client, client_id, client_secret, proxy, title=title, artist=artist
        )
        if result:
            return result
        logger.debug("Spotify API search returned nothing, falling back to DDG")

    return await _search_via_ddg(query, client, proxy, title=title, artist=artist)


async def _search_via_api(
    query: str,
    client: httpx.AsyncClient,
    client_id: str,
    client_secret: str,
    proxy: str | None,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        from music_library.parsers.spotify import _get_access_token
        token = await _get_access_token(client_id, client_secret, proxy, timeout=8.0)
        kwargs: dict = {}
        if proxy:
            kwargs["proxy"] = proxy
        async with httpx.AsyncClient(timeout=10, follow_redirects=True, **kwargs) as c:
            resp = await c.get(
                "https://api.spotify.com/v1/search",
                params={"q": query, "type": "track", "limit": "5"},
                headers={"Authorization": f"Bearer {token}"},
            )
            resp.raise_for_status()
            data = resp.json()

        items = data.get("tracks", {}).get("items", [])
        best_url, best_score = None, 0.0
        for item in items:
            c_title = item.get("name", "")
            c_artist = ", ".join(a["name"] for a in item.get("artists", []))
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("Spotify API score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score = s
                best_url = item.get("external_urls", {}).get("spotify")
        if best_score >= _MIN_SCORE:
            return best_url
        logger.debug("Spotify API best score %.2f below threshold", best_score)
        return None
    except Exception as exc:
        logger.warning("Spotify API search failed: %s", exc)
        return None


async def _search_via_ddg(
    query: str,
    client: httpx.AsyncClient,
    proxy: str | None,
    title: str = "",
    artist: str = "",
) -> str | None:
    try:
        kwargs: dict = {}
        if proxy:
            kwargs["proxy"] = proxy
        async with httpx.AsyncClient(follow_redirects=True, timeout=10, **kwargs) as c:
            resp = await c.get(
                _DDG_URL,
                params={"q": f"site:open.spotify.com/track {query}"},
                headers={"Accept": "text/html", "User-Agent": "Mozilla/5.0"},
            )
        urls: list[str] = []
        seen: set[str] = set()
        for m in _TRACK_URL_RE.finditer(resp.text):
            url = m.group(0)
            if url not in seen:
                seen.add(url)
                urls.append(url)
                if len(urls) >= 5:
                    break

        if not urls:
            logger.debug("Spotify DDG: no results for %r", query)
            return None

        if not title and not artist:
            logger.debug("Spotify DDG result (no scoring): %s", urls[0])
            return urls[0]

        best_url, best_score = None, 0.0
        async with httpx.AsyncClient(follow_redirects=True, timeout=8) as oc:
            for url in urls:
                try:
                    r = await oc.get(f"https://open.spotify.com/oembed?url={url}")
                    if r.status_code != 200:
                        continue
                    d = r.json()
                    oembed_title = d.get("title", "")
                    parts = oembed_title.split(" - ", 1)
                    c_title = parts[0].strip() if parts else oembed_title
                    c_artist = parts[1].strip() if len(parts) > 1 else ""
                    s = track_score(c_artist, c_title, artist, title)
                    logger.debug("Spotify DDG+oEmbed score=%.2f url=%s", s, url)
                    if s > best_score:
                        best_score = s
                        best_url = url
                except Exception:
                    continue

        if best_score >= _MIN_SCORE:
            return best_url
        return urls[0] if urls else None

    except Exception as exc:
        logger.warning("Spotify DDG search failed: %s", exc)
        return None
