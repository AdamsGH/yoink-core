"""Anonymous Yandex search with legacy scoring."""
from music_library.utils import track_score
async def search(query, client, title="", artist=""):
    response = await client.get("https://api.music.yandex.net/search", params={"text": f"{artist} - {title}" if artist and title else query, "type": "track", "page": 0})
    response.raise_for_status()
    tracks = (response.json().get("result", {}).get("tracks") or {}).get("results", [])
    for track in tracks[:5]:
        name = ", ".join(a["name"] for a in track.get("artists", []))
        if track_score(name, track.get("title") or "", artist, title) >= .6 and track.get("albums"):
            return f"https://music.yandex.ru/album/{track['albums'][0]['id']}/track/{track['id']}"
    return None
