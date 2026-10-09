"""YouTube Music search adapter - ytmusicapi with ytsearch fallback."""
from __future__ import annotations

import asyncio
import logging
from concurrent.futures import ThreadPoolExecutor
from typing import TYPE_CHECKING

from music_library.utils import track_score

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

_MIN_SCORE = 0.6
_CYR_RE = __import__('re').compile(r'[\u0430-\u044f\u0451\u0410-\u042f\u0401]')
_executor = ThreadPoolExecutor(max_workers=2, thread_name_prefix="ytmusic")


def _to_cyrillic(s: str) -> str | None:
    """Transliterate Latin -> Cyrillic. Returns None if already has Cyrillic or fails."""
    if _CYR_RE.search(s):
        return None
    try:
        from transliterate import translit
        result = translit(s, 'ru')
        return result if result != s else None
    except Exception:
        return None


async def search(
    query: str,
    client: httpx.AsyncClient,
    title: str = "",
    artist: str = "",
    proxy: str | None = None,
) -> str | None:
    # Build a Cyrillic variant of the query: ytmusicapi works better with
    # full-Cyrillic queries when the source platform (Spotify) provides
    # Latin artist names (e.g. 'Monetochka' vs 'Монеточка').
    cyr_artist = _to_cyrillic(artist)
    cyr_query = f"{cyr_artist} {title}" if cyr_artist and title else None

    # Run original and Cyrillic queries in parallel; take first hit.
    queries = [query]
    if cyr_query and cyr_query != query:
        queries.append(cyr_query)

    tasks = [
        asyncio.create_task(
            _search_ytmusicapi(q, title=title, artist=artist, proxy=proxy)
        )
        for q in queries
    ]
    result = None
    for task in asyncio.as_completed(tasks):
        r = await task
        if r and not result:
            result = r
    for task in tasks:
        task.cancel()

    if result:
        return result
    logger.info("YTMusic API found nothing for %r, falling back to ytsearch", query)
    return await _search_ytsearch(query, title=title, artist=artist, proxy=proxy)


async def _search_ytmusicapi(
    query: str,
    title: str = "",
    artist: str = "",
    proxy: str | None = None,
) -> str | None:
    try:
        from ytmusicapi import YTMusic
        loop = asyncio.get_running_loop()
        results = await loop.run_in_executor(
            _executor,
            lambda: YTMusic(
                proxies={"http": proxy, "https": proxy} if proxy else None
            ).search(query, filter="songs", limit=10),
        )
        if not results:
            return None
        for item in results:
            c_title = item.get("title", "")
            c_artist = " ".join(a.get("name", "") for a in item.get("artists", []))
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("YTMusic score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s >= _MIN_SCORE:
                vid = item.get("videoId")
                if vid:
                    return f"https://music.youtube.com/watch?v={vid}"
        return None
    except ImportError:
        return None
    except Exception as exc:
        logger.debug("YTMusic API search failed: %s", exc)
        return None


async def _search_ytsearch(
    query: str,
    title: str = "",
    artist: str = "",
    proxy: str | None = None,
) -> str | None:
    """Fallback: yt-dlp ytsearch on regular YouTube, score candidates.

    Returns a music.youtube.com link so it still shows as YouTube Music
    in the card - the video is playable from either domain.
    """
    try:
        from music_library import ytdlp as yt_dlp
        loop = asyncio.get_running_loop()

        def _search():
            opts = {"quiet": True, "extract_flat": True}
            if proxy:
                opts["proxy"] = proxy
            with yt_dlp.YoutubeDL(opts) as ydl:
                info = ydl.extract_info(f"ytsearch5:{query}", download=False)
            return (info or {}).get("entries", [])

        entries = await loop.run_in_executor(_executor, _search)
        best_url, best_score = None, 0.0
        for entry in entries:
            vid = entry.get("id") or entry.get("url", "").split("v=")[-1].split("&")[0]
            raw_title = entry.get("title", "")
            c_channel = entry.get("channel") or entry.get("uploader", "")
            # YouTube video titles are often "Artist - Track Title (suffix)"
            # Try splitting on " - " and score both interpretations, take the best.
            c_title, c_artist = raw_title, c_channel
            if " - " in raw_title:
                parts = raw_title.split(" - ", 1)
                c_artist_from_title = parts[0].strip()
                c_title_from_title = parts[1].strip()
                s_split = track_score(c_artist_from_title, c_title_from_title, artist, title)
                s_raw = track_score(c_channel, raw_title, artist, title)
                if s_split >= s_raw:
                    c_artist, c_title = c_artist_from_title, c_title_from_title
            s = track_score(c_artist, c_title, artist, title)
            logger.debug("ytsearch score=%.2f artist=%r title=%r", s, c_artist, c_title)
            if s > best_score:
                best_score = s
                best_url = f"https://music.youtube.com/watch?v={vid}" if vid else None
        if best_score >= _MIN_SCORE and best_url:
            logger.info("ytsearch fallback hit: score=%.2f url=%s", best_score, best_url)
            return best_url
        logger.info("ytsearch fallback: best score %.2f below threshold", best_score)
        return None
    except Exception as exc:
        logger.warning("ytsearch fallback failed: %s", exc)
        return None
