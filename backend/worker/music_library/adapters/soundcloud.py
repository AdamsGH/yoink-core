"""SoundCloud search adapter - api-v2.soundcloud.com/search/tracks.

API calls go through httpx (system OpenSSL), NOT curl_cffi. BoringSSL inside
curl_cffi handshakes get rejected by Cloudflare on api-v2.soundcloud.com when
tunneled through SOCKS5, surfacing as 'curl: (35) invalid library'. The public
API does not enforce browser-TLS fingerprinting, so a plain httpx client works.
Client_id discovery still uses curl_cffi (see parsers.soundcloud) because the
soundcloud.com homepage IS fingerprint-gated.
"""
from __future__ import annotations

import logging

from music_library import transport as httpx

from music_library.parsers.soundcloud import _fetch_client_id
from music_library.utils import track_score

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.5


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
    proxy: str | None = None,
) -> str | None:
    try:
        client_id = await _fetch_client_id(proxy)
        if not client_id:
            logger.debug("SoundCloud search: no client_id available")
            return None

        api = _api_client(client, proxy)
        try:
            resp = await api.get(
                "https://api-v2.soundcloud.com/search/tracks",
                params={"q": query, "limit": "5", "client_id": client_id},
                timeout=15,
            )
        finally:
            if api is not client:
                await api.aclose()
        if resp.status_code == 401:
            logger.debug("SoundCloud search: client_id expired")
            return None
        resp.raise_for_status()
        data = resp.json()

        items = data.get("collection", [])
        if not items:
            return None

        best_url: str | None = None
        best_score = 0.0
        for item in items:
            c_title = (item.get("title") or "").strip()
            c_artist = (item.get("user") or {}).get("username", "").strip()
            url = item.get("permalink_url", "")
            if not url:
                continue
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("SoundCloud score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score, best_url = s, url

        return best_url if best_score >= _MIN_SCORE else None
    except Exception as exc:
        logger.debug("SoundCloud search failed: %s", exc)
        return None


def _api_client(base: httpx.AsyncClient, proxy: str | None) -> httpx.AsyncClient:
    if not proxy:
        return base
    return httpx.AsyncClient(proxy=proxy, timeout=base.timeout, follow_redirects=True)
