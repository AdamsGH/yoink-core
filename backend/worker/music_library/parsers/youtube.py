"""YouTube parser - extracts track info from a regular YouTube music video.

Strategy:
  1. yt-dlp extract_info (no download) for title, channel, tags, category
  2. Parse title with 'Artist - Track (suffix)' regex
  3. If no dash separator, fall back to channel name as artist
  4. Strip feat./ft. from artist for cleaner search queries
"""
from __future__ import annotations

import asyncio
import logging
import re
from concurrent.futures import ThreadPoolExecutor
from typing import TYPE_CHECKING

from music_library.types import ResolverError

if TYPE_CHECKING:
    from music_library import transport as httpx

logger = logging.getLogger(__name__)

# Matches both youtube.com and youtu.be, excluding music.youtube.com
# (that is handled by ytmusic parser)
TRACK_RE = re.compile(
    r"https?://(?:www\.)?(?:youtube\.com/watch\?[^\s]*v=|youtu\.be/)[A-Za-z0-9_-]{11}",
    re.IGNORECASE,
)

# Title patterns to strip from the track name
_STRIP_SUFFIXES = re.compile(
    r"\s*[\(\[]\s*(?:official\s*(?:video|audio|music\s*video|lyric\s*video|hd|hq|mv)?|"
    r"lyric(?:s)?\s*(?:video)?|music\s*video|hd|hq|4k|audio|visualizer|remaster(?:ed)?|"
    r"full\s*album|live|acoustic|karaoke|instrumental|radio\s*edit|extended\s*(?:mix)?|"
    r"clip\s*officiel|video\s*clip)[^\)\]]*[\)\]]\s*",
    re.IGNORECASE,
)

# 'Artist feat. X - Track' or 'Artist - Track' separator
_DASH_SPLIT = re.compile(r"^(.+?)\s+-\s+(.+)$")

# Strip feat./ft. from artist string
_FEAT_RE = re.compile(
    r"\s+(?:feat\.?|ft\.?|featuring|with)\s+.+$",
    re.IGNORECASE,
)

_executor = ThreadPoolExecutor(max_workers=2, thread_name_prefix="yt_info")


def _extract_video_id(url: str) -> str | None:
    m = re.search(r"(?:v=|youtu\.be/)([A-Za-z0-9_-]{11})", url)
    return m.group(1) if m else None


def _parse_title(raw_title: str) -> tuple[str, str]:
    """Return (artist, track) from a raw YouTube title.

    Returns ('', raw_title) if no artist separator found.
    """
    m = _DASH_SPLIT.match(raw_title.strip())
    if not m:
        return "", _STRIP_SUFFIXES.sub("", raw_title).strip()

    artist_raw = m.group(1).strip()
    track_raw = _STRIP_SUFFIXES.sub("", m.group(2)).strip()
    # Remove trailing junk left after suffix strip
    track_raw = re.sub(r"\s{2,}", " ", track_raw).strip()

    # Strip feat. from artist for cleaner cross-platform search
    artist_clean = _FEAT_RE.sub("", artist_raw).strip()

    return artist_clean, track_raw


def _run_ytdlp(video_id: str, proxy: str | None) -> dict:
    try:
        from music_library import ytdlp as yt_dlp
    except ImportError as exc:
        raise ResolverError("yt-dlp is not installed") from exc

    opts: dict = {
        "quiet": True,
        "no_warnings": True,
        "skip_download": True,
    }
    if proxy:
        opts["proxy"] = proxy

    url = f"https://www.youtube.com/watch?v={video_id}"
    with yt_dlp.YoutubeDL(opts) as ydl:
        try:
            return ydl.extract_info(url, download=False) or {}
        except Exception as exc:
            raise ResolverError(f"yt-dlp failed: {exc}") from exc


async def parse(
    url: str,
    client: httpx.AsyncClient,
    *,
    proxy: str | None = None,
) -> tuple[str, str, str | None]:
    """Return (title, artist, thumbnail_url).

    Raises ResolverError if the video is not in the Music category or
    title parsing yields no usable track name.
    """
    video_id = _extract_video_id(url)
    if not video_id:
        raise ResolverError(f"Cannot extract YouTube video ID from {url}")

    loop = asyncio.get_running_loop()
    info = await loop.run_in_executor(_executor, _run_ytdlp, video_id, proxy)

    categories = info.get("categories") or []
    if "Music" not in categories:
        raise ResolverError(
            f"YouTube video {video_id} is not in Music category (got: {categories})"
        )

    raw_title = info.get("title") or ""
    if not raw_title:
        raise ResolverError(f"YouTube video {video_id} has no title")

    # yt-dlp sometimes provides these for music videos
    yt_artist = info.get("artist") or ""
    yt_track = info.get("track") or ""

    if yt_artist and yt_track:
        artist, track = yt_artist, yt_track
    else:
        artist, track = _parse_title(raw_title)
        if not artist:
            # Fall back to channel name stripped of common suffixes
            channel = info.get("channel") or info.get("uploader") or ""
            artist = re.sub(
                r"\s*(?:VEVO|Music|Official|Records?|Recordings?|Entertainment)\s*$",
                "",
                channel,
                flags=re.IGNORECASE,
            ).strip()

    if not track:
        raise ResolverError(f"Could not extract track name from YouTube title: {raw_title!r}")

    thumbnails = info.get("thumbnails") or []
    thumbnail: str | None = None
    for t in reversed(thumbnails):
        if t.get("url"):
            thumbnail = t["url"]
            break
    if not thumbnail:
        thumbnail = info.get("thumbnail")

    logger.info(
        "YouTube parsed: id=%s artist=%r track=%r category=Music",
        video_id, artist, track,
    )
    return track, artist, thumbnail
