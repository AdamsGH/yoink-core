"""SoundCloud parser.

Client ID discovery (two-phase, same approach as cobalt):
  1. Fetch soundcloud.com homepage, try hydration JSON: "hydratable":"apiClient","data":{"id":"..."}
  2. Fallback: scrape JS bundle URLs from the page, grep for ,client_id:"<32chars>",

Client ID is cached by SC version string (window.__sc_version).
Once we have a client_id we resolve the track via api-v2.soundcloud.com/resolve
and return title/artist/thumbnail from the JSON response.

oEmbed fallback: if client_id extraction fails entirely, hit soundcloud.com/oembed
which returns title (no artist) without authentication.
"""
from __future__ import annotations

import asyncio
import logging
import re
from dataclasses import dataclass

from music_library import transport as httpx
from music_library.transport import AsyncSession

from music_library.types import ResolverError

logger = logging.getLogger(__name__)

TRACK_RE = re.compile(r"soundcloud\.com/([\w-]+)/([\w-]+)(?:[?#]|$)")

_VERSION_RE = re.compile(r'window\.__sc_version="(\d{10})"')
_HYDRATION_ID_RE = re.compile(r'"hydratable"\s*:\s*"apiClient"\s*,\s*"data"\s*:\s*\{\s*"id"\s*:\s*"([^"]+)"')
_BUNDLE_ID_RE = re.compile(r',client_id:"([A-Za-z0-9]{32})",')
_BUNDLE_URL_RE = re.compile(r'<script[^>]+src="(https://a-v2\.sndcdn\.com/[^"]+)"')


@dataclass
class _Cache:
    version: str = ""
    client_id: str = ""


_cache = _Cache()
_lock = asyncio.Lock()


def _make_session(proxy: str | None) -> AsyncSession:
    """curl_cffi session for soundcloud.com homepage / JS bundles.

    Browser-TLS fingerprinting is enforced by Cloudflare on the public site
    (homepage HTML, a-v2.sndcdn.com bundles). API hosts (api-v2.soundcloud.com,
    soundcloud.com/oembed) reject BoringSSL handshakes when tunneled through
    SOCKS5, so those go through httpx instead via _make_api_client().
    """
    s = AsyncSession(impersonate="chrome")
    if proxy:
        s.proxies = {"https": proxy, "http": proxy}
    return s


def _make_api_client(proxy: str | None) -> httpx.AsyncClient:
    """httpx client for api-v2.soundcloud.com (system OpenSSL, no impersonate)."""
    return httpx.AsyncClient(proxy=proxy, follow_redirects=True, timeout=15)


async def _fetch_client_id(proxy: str | None) -> str | None:
    async with _make_session(proxy) as session:
        try:
            resp = await session.get("https://soundcloud.com/", allow_redirects=True, timeout=15)
            resp.raise_for_status()
            html = resp.text
        except Exception as exc:
            logger.debug("SoundCloud homepage fetch failed: %s", exc)
            return None

        version_m = _VERSION_RE.search(html)
        version = version_m.group(1) if version_m else ""

        async with _lock:
            if version and _cache.version == version and _cache.client_id:
                logger.debug("SoundCloud client_id cache hit (version=%s)", version)
                return _cache.client_id

        # Phase 1: hydration JSON
        hydration_m = _HYDRATION_ID_RE.search(html)
        if hydration_m:
            client_id = hydration_m.group(1)
            logger.debug("SoundCloud client_id from hydration: %s", client_id)
            async with _lock:
                _cache.version = version
                _cache.client_id = client_id
            return client_id

        # Phase 2: JS bundle scrape
        bundle_urls = _BUNDLE_URL_RE.findall(html)
        for url in bundle_urls:
            try:
                r = await session.get(url, allow_redirects=True, timeout=10)
                m = _BUNDLE_ID_RE.search(r.text)
                if m:
                    client_id = m.group(1)
                    logger.debug("SoundCloud client_id from bundle %s: %s", url, client_id)
                    async with _lock:
                        _cache.version = version
                        _cache.client_id = client_id
                    return client_id
            except Exception as exc:
                logger.debug("SoundCloud bundle fetch failed (%s): %s", url, exc)

    logger.warning("SoundCloud: could not extract client_id")
    return None


async def _resolve_via_api(url: str, client_id: str, proxy: str | None) -> tuple[str, str, str | None]:
    async with _make_api_client(proxy) as session:
        resp = await session.get(
            "https://api-v2.soundcloud.com/resolve",
            params={"url": url, "client_id": client_id},
            timeout=15,
        )
        if resp.status_code == 401:
            # Stale client_id - invalidate cache
            async with _lock:
                _cache.client_id = ""
            raise ResolverError("SoundCloud: client_id expired (401)")
        resp.raise_for_status()
        data = resp.json()

    title = (data.get("title") or "").strip()
    artist = (data.get("user") or {}).get("username", "").strip()
    artwork = data.get("artwork_url") or (data.get("user") or {}).get("avatar_url")
    if artwork:
        artwork = artwork.replace("-large", "-t500x500")

    if not title:
        raise ResolverError(f"SoundCloud API: empty title for {url}")

    return title, artist, artwork


async def _oembed_fallback(url: str, proxy: str | None) -> tuple[str, str, str | None]:
    """Last resort: oEmbed gives title only, no artist."""
    async with _make_api_client(proxy) as session:
        resp = await session.get(
            "https://soundcloud.com/oembed",
            params={"url": url, "format": "json"},
            timeout=10,
        )
        resp.raise_for_status()
        data = resp.json()

    title = (data.get("title") or "").strip()
    if not title:
        raise ResolverError(f"SoundCloud oEmbed: empty title for {url}")

    # oEmbed title format: "Track by Artist" or just "Track"
    artist = ""
    m = re.match(r"^(.+)\s+by\s+(.+)$", title, re.IGNORECASE)
    if m:
        title = m.group(1).strip()
        artist = m.group(2).strip()

    thumbnail = data.get("thumbnail_url")
    return title, artist, thumbnail


async def parse(url: str, proxy: str | None = None) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url)."""
    client_id = await _fetch_client_id(proxy)

    if client_id:
        try:
            return await _resolve_via_api(url, client_id, proxy)
        except ResolverError:
            raise
        except Exception as exc:
            logger.debug("SoundCloud API resolve failed, trying oEmbed: %s", exc)

    logger.info("SoundCloud: falling back to oEmbed for %s", url)
    try:
        return await _oembed_fallback(url, proxy)
    except Exception as exc:
        raise ResolverError(f"SoundCloud: all methods failed for {url}: {exc}") from exc
