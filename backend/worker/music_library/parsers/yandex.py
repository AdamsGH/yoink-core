"""Anonymous Yandex metadata through the bounded transport."""
import re
from music_library.types import ResolverError
TRACK_RE = re.compile(r"music\.yandex\.[a-z]+/album/(\d+)/track/(\d+)")
async def parse(url, client):
    match = TRACK_RE.search(url)
    if not match:
        raise ResolverError("Invalid Yandex track")
    response = await client.get("https://api.music.yandex.net/tracks/" + match[2] + ":" + match[1])
    response.raise_for_status()
    tracks = response.json().get("result") or []
    if not tracks:
        raise ResolverError("Yandex track not found")
    track = tracks[0]
    cover = track.get("coverUri")
    return track.get("title") or "", ", ".join(a["name"] for a in track.get("artists", [])), "https://" + cover.replace("%%", "400x400") if cover else None
