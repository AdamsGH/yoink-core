"""Artist page resolver: Spotify artist URL -> ArtistInfo with cross-platform links."""
from __future__ import annotations

import asyncio
import logging
import re
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING

from music_library.parsers.spotify import _get_access_token, _make_client
from music_library.types import ArtistInfo, ArtistTrack, ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

SPOTIFY_ARTIST_RE = re.compile(
    r"open\.spotify\.com/(?:intl-[a-z]{2}/)?artist/([A-Za-z0-9]+)", re.IGNORECASE
)


async def resolve_spotify_artist(
    url: str,
    http_client: httpx.AsyncClient,
    *,
    client_id: str | None = None,
    client_secret: str | None = None,
    proxy: str | None = None,
) -> ArtistInfo:
    m = SPOTIFY_ARTIST_RE.search(url)
    if not m:
        raise ResolverError(f"Cannot extract Spotify artist ID from {url}")
    artist_id = m.group(1)

    if not client_id or not client_secret:
        raise ResolverError("Spotify API credentials required for artist lookup")

    timeout = http_client.timeout.read or 8
    token = await _get_access_token(client_id, client_secret, proxy, timeout)
    client = _make_client(proxy, timeout) if proxy else http_client

    try:
        async with (client if proxy else _nullctx(client)) as c:
            artist_resp, top_resp = await asyncio.gather(
                c.get(
                    f"https://api.spotify.com/v1/artists/{artist_id}",
                    headers={"Authorization": f"Bearer {token}"},
                ),
                c.get(
                    f"https://api.spotify.com/v1/artists/{artist_id}/top-tracks",
                    headers={"Authorization": f"Bearer {token}"},
                    params={"market": "US"},
                ),
            )
        artist_resp.raise_for_status()
        top_resp.raise_for_status()
    except Exception as exc:
        raise ResolverError(f"Spotify artist API failed: {exc}") from exc

    artist_data = artist_resp.json()
    top_data = top_resp.json()

    name = artist_data.get("name") or ""
    if not name:
        raise ResolverError("Spotify artist API returned empty name")

    genres = artist_data.get("genres") or []
    images = artist_data.get("images") or []
    thumbnail = images[0]["url"] if images else None

    spotify_link = artist_data.get("external_urls", {}).get("spotify") or url

    top_tracks: list[ArtistTrack] = []
    for track in (top_data.get("tracks") or [])[:5]:
        track_url = track.get("external_urls", {}).get("spotify") or ""
        top_tracks.append(ArtistTrack(
            title=track.get("name") or "",
            links=[("spotify", "Spotify", track_url)] if track_url else [],
        ))

    info = ArtistInfo(
        name=name,
        genres=genres[:4],
        thumbnail_url=thumbnail,
        source_url=url,
        platform_links=[("spotify", "Spotify", spotify_link)],
        top_tracks=top_tracks,
    )

    # Enrich artist page links + cross-link tracks on all platforms
    await _enrich_all(info, http_client, artist_name=name)

    # Cross-link tracks on platforms not yet covered by _enrich_all
    await _enrich_tracks(info.top_tracks, name, http_client)

    return info


_PLATFORM_NAMES = {
    "deezer": "Deezer",
    "yandex": "Yandex Music",
    "ytmusic": "YouTube Music",
}


async def _enrich_all(info: ArtistInfo, client: httpx.AsyncClient, artist_name: str = "") -> None:
    """Enrich artist info with links from all available platforms in parallel."""
    name = artist_name or info.name
    tasks = {
        "deezer": asyncio.create_task(_search_deezer_artist(name, info.top_tracks, client)),
        "yandex": asyncio.create_task(_search_yandex_artist(name, client)),
        "ytmusic": asyncio.create_task(_search_ytmusic_artist(name, client)),
    }
    results: dict[str, str | None] = {}
    for key, task in tasks.items():
        try:
            results[key] = await task
        except Exception as exc:
            logger.debug("Artist enrich %s failed for %r: %s", key, name, exc)
            results[key] = None

    for key, url in results.items():
        if url:
            info.platform_links.append((key, _PLATFORM_NAMES[key], url))


async def _enrich_tracks(
    tracks: list,
    artist_name: str,
    client: httpx.AsyncClient,
) -> None:
    """Add cross-platform links to each ArtistTrack via adapters (parallel per track)."""
    from music_library.adapters import deezer as deezer_adapter
    from music_library.adapters import yandex as yandex_adapter
    from music_library.adapters import ytmusic as ytmusic_adapter

    adapter_map = [
        ("deezer", "Deezer", deezer_adapter.search),
        ("yandex", "Yandex Music", yandex_adapter.search),
        ("ytmusic", "YouTube Music", ytmusic_adapter.search),
    ]

    async def _enrich_one(track: ArtistTrack) -> None:
        existing_keys = {k for k, _, _ in track.links}
        query = f"{artist_name} {track.title}"
        tasks = {
            key: asyncio.create_task(fn(query, client, title=track.title, artist=artist_name))
            for key, _, fn in adapter_map
            if key not in existing_keys
        }
        for key, display_name, _ in adapter_map:
            task = tasks.get(key)
            if task is None:
                continue
            try:
                url = await task
                if url:
                    track.links.append((key, display_name, url))
            except Exception as exc:
                logger.debug("Track adapter %s failed for %r: %s", key, track.title, exc)

    await asyncio.gather(*[_enrich_one(t) for t in tracks], return_exceptions=True)


async def _search_deezer_artist(
    name: str,
    top_tracks: list,
    client: httpx.AsyncClient,
) -> str | None:
    resp = await client.get(
        "https://api.deezer.com/search/artist",
        params={"q": name, "limit": 1},
        timeout=6,
    )
    data = resp.json()
    artists = data.get("data") or []
    if not artists:
        return None
    deezer_artist = artists[0]
    deezer_artist_id = deezer_artist.get("id")
    artist_url = deezer_artist.get("link") or f"https://www.deezer.com/artist/{deezer_artist_id}"

    if deezer_artist_id and top_tracks:
        try:
            top_resp = await client.get(
                f"https://api.deezer.com/artist/{deezer_artist_id}/top",
                params={"limit": 5},
                timeout=6,
            )
            deezer_tracks = top_resp.json().get("data") or []
            for i, track in enumerate(top_tracks):
                if i >= len(deezer_tracks):
                    break
                dz = deezer_tracks[i]
                dz_url = dz.get("link") or f"https://www.deezer.com/track/{dz.get('id')}"
                track.links.append(("deezer", "Deezer", dz_url))
        except Exception as exc:
            logger.debug("Deezer top tracks failed: %s", exc)

    return artist_url


async def _search_yandex_artist(name: str, client: httpx.AsyncClient) -> str | None:
    response = await client.get("https://api.music.yandex.net/search", params={"text": name, "type": "artist"})
    response.raise_for_status()
    artists = (response.json().get("result", {}).get("artists") or {}).get("results", [])
    return f"https://music.yandex.ru/artist/{artists[0]['id']}" if artists else None


async def _search_ytmusic_artist(name: str, client: httpx.AsyncClient) -> str | None:
    try:
        from ytmusicapi import YTMusic
        yt = YTMusic()
        results = yt.search(name, filter="artists", limit=1)
        if not results:
            return None
        browse_id = results[0].get("browseId")
        if not browse_id:
            return None
        return f"https://music.youtube.com/channel/{browse_id}"
    except ImportError:
        return None


@asynccontextmanager
async def _nullctx(client: httpx.AsyncClient):
    yield client
