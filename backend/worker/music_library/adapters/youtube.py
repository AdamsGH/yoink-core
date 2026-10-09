"""Audio-only fallback. Native cross-provider cards never search YouTube."""
import asyncio
import re
from music_library.ytdlp import YoutubeDL

async def search(query, client, title="", artist="", proxy=None):
    def lookup():
        with YoutubeDL({"quiet": True, "extract_flat": True, "proxy": proxy or ""}) as downloader:
            info = downloader.extract_info("ytsearch1:" + query, download=False)
        entries = (info or {}).get("entries") or []
        if not entries:
            return None
        video = entries[0].get("id")
        return "https://www.youtube.com/watch?v=" + video if isinstance(video, str) and re.fullmatch(r"[A-Za-z0-9_-]{11}", video) else None
    return await asyncio.to_thread(lookup)
