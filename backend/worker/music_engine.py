"""Confined provider functions and tagged MP3 artifacts, not a bot or service."""
import asyncio
import importlib
import json
from pathlib import Path
from urllib.parse import urlsplit
from failures import MediaFailure
import network_guard as guard
from proxy_tunnel import proxy_route
from music_library import transport

PROVIDERS = {"spotify", "deezer", "yandex", "ytmusic", "soundcloud", "apple_music", "bandcamp", "qobuz", "tidal", "youtube"}
TASKS = {"music_parse", "music_search", "music_artist", "music_audio"}

def validate(r):
    import re
    if not isinstance(r, dict) or set(r) != {"version", "operation_id", "attempt", "task", "proxy_url", "credentials", "request_timeout_ms", "response_max_bytes", "max_requests"}:
        raise ValueError("invalid music request")
    if type(r["version"]) is not int or r["version"] != 2 or not isinstance(r["operation_id"], str) or not re.fullmatch(r"[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", r["operation_id"]) or not int(r["operation_id"].replace("-", ""), 16) or type(r["attempt"]) is not int or not 1 <= r["attempt"] <= 0xffffffff:
        raise ValueError("invalid music identity")
    for key, low, high in (("request_timeout_ms", 1, 120000), ("response_max_bytes", 1024, 8388608), ("max_requests", 1, 100)):
        if type(r[key]) is not int or not low <= r[key] <= high:
            raise ValueError("invalid music budget")
    c = r["credentials"]
    if not isinstance(c, dict) or set(c) != {"spotify_client_id", "spotify_client_secret", "tidal_client_id", "tidal_client_secret"}:
        raise ValueError("invalid credentials")
    for value in c.values():
        if value is not None:
            clean(value, 4096)
    if r["proxy_url"] is not None:
        guard._parse_http_url(r["proxy_url"], "invalid proxy", allow_credentials=True)
    task = r["task"]
    if not isinstance(task, dict) or task.get("kind") not in TASKS:
        raise ValueError("invalid music task")
    expected = {"music_parse": {"kind", "provider", "url"}, "music_search": {"kind", "provider", "query", "title", "artist"}, "music_artist": {"kind", "url"}, "music_audio": {"kind", "url", "title", "artist", "thumbnail_url"}}[task["kind"]]
    if set(task) != expected or ("provider" in task and task["provider"] not in PROVIDERS):
        raise ValueError("invalid music task")
    for field in ("title", "artist", "query"):
        if field in task:
            clean(task[field], 2048 if field == "query" else 1024)
    if "url" in task:
        guard._parse_http_url(task["url"], "invalid source")
    if task.get("thumbnail_url"):
        guard._parse_http_url(task["thumbnail_url"], "invalid cover")
    return r

def clean(value, maximum):
    if not isinstance(value, str) or len(value.encode()) > maximum or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError("invalid music string")
    return value

async def provider_task(task, credentials, proxy):
    async with transport.AsyncClient(proxy=proxy, follow_redirects=True) as client:
        kind = task["kind"]
        if kind == "music_artist":
            from music_library.parsers.artist import resolve_spotify_artist
            from dataclasses import asdict
            result = await resolve_spotify_artist(task["url"], client, client_id=credentials["spotify_client_id"], client_secret=credentials["spotify_client_secret"], proxy=proxy)
            data = asdict(result)
            data.pop("source_url")
            return {"kind": "artist", **data}
        provider = task["provider"]
        module = importlib.import_module("music_library." + ("parsers." if kind == "music_parse" else "adapters.") + provider)
        kwargs = {}
        if provider in {"spotify", "tidal"}:
            # Tidal's public parser needs no client credentials.
            if provider == "spotify" or kind == "music_search":
                kwargs.update(client_id=credentials[provider + "_client_id"], client_secret=credentials[provider + "_client_secret"])
        if provider in {"spotify", "soundcloud", "ytmusic", "youtube"}:
            if provider != "ytmusic" or kind == "music_search":
                kwargs["proxy"] = proxy
        if kind == "music_parse":
            args = [task["url"]] if provider == "soundcloud" else [task["url"], client]
            title, artist, cover = await module.parse(*args, **kwargs)
            clean(title, 1024)
            clean(artist or "", 1024)
            if not title.strip():
                raise MediaFailure("invalid_media")
            return {"kind": "metadata", "metadata": {"title": title, "artist": artist or None, "thumbnail_url": cover}}
        url = await module.search(task["query"], client, title=task["title"], artist=task["artist"], **kwargs)
        return {"kind": "link", "url": url}

def audio(task, proxy, settings):
    from media_engine import download_media
    from main import _validate_request
    from mutagen.id3 import ID3, TIT2, TPE1, APIC, ID3NoHeaderError
    from music_library.ytdlp import AudioYoutubeDL
    if urlsplit(task["url"]).hostname not in ("youtube.com", "www.youtube.com", "music.youtube.com", "youtu.be"):
        raise MediaFailure("unsupported_source")
    request = _validate_request(json.dumps({"version": 2, "operation_id": "11111111-1111-1111-1111-111111111111", "attempt": 1, "source_url": task["url"], "task": {"kind": "download", "media_mode": "audio"}, "options": {"audio_codec": "mp3"}}).encode())
    request["network"] = {"proxy_url": proxy}
    request["options"]["_music_audio_bitrate"] = 192
    artifacts = download_media(request, settings, lambda _: None, youtube_dl_factory=AudioYoutubeDL)
    if len(artifacts) != 1 or artifacts[0]["media_type"] != "audio/mpeg":
        raise MediaFailure("invalid_media")
    artifact = artifacts[0]
    path = Path(artifact["path"])
    try:
        tags = ID3(path)
    except ID3NoHeaderError:
        tags = ID3()
    tags["TIT2"] = TIT2(encoding=3, text=task["title"])
    tags["TPE1"] = TPE1(encoding=3, text=task["artist"])
    if task.get("thumbnail_url"):
        async def cover():
            async with transport.AsyncClient(proxy=proxy, follow_redirects=True) as client:
                return await client.get(task["thumbnail_url"])
        try:
            response = asyncio.run(cover())
            response.raise_for_status()
            data = response.content
            mime = "image/jpeg" if data.startswith(b"\xff\xd8\xff") else "image/png" if data.startswith(b"\x89PNG\r\n\x1a\n") else None
            if mime:
                tags["APIC"] = APIC(encoding=3, mime=mime, type=3, desc="Cover", data=data)
        except Exception:
            pass
    tags.save(path)
    artifact["size_bytes"] = path.stat().st_size
    if artifact["size_bytes"] > min(settings.max_bytes, 49 * 1024 * 1024):
        raise MediaFailure("resource_limit")
    return artifacts

def execute(request, settings, emit, prefix):
    before = guard.guard_failure_count()
    try:
        # Music never inherits the content fixture's private-source allowances.
        transport.configure(min(request["request_timeout_ms"] / 1000, settings.socket_timeout_secs), request["response_max_bytes"], request["max_requests"])
        with guard.guard_context((), settings.allowed_proxy_origins, source_wire_admission=transport._budget.reserve), transport.requests_budget():
            guard.configure_proxy(request["proxy_url"])
            with proxy_route(request["proxy_url"], settings.socket_timeout_secs) as proxy:
                transport._budget.proxy = proxy
                task = request["task"]
                if task["kind"] == "music_audio":
                    artifacts = audio(task, proxy, settings)
                    event = {"type": "completed", "artifacts": artifacts}
                else:
                    result = asyncio.run(provider_task(task, request["credentials"], proxy))
                    validate_result_urls(result)
                    event = {"type": "music_completed", "result": result}
            if guard.guard_failure_count() != before:
                raise MediaFailure("source_unavailable")
        emit({**prefix(request), "event": event})
    except BaseException as exc:
        from failures import classify_error
        emit({**prefix(request), "event": {"type": "failure", "class": classify_error(exc)}})

def validate_result_urls(result):
    urls = [result.get("url"), result.get("thumbnail_url"), result.get("metadata", {}).get("thumbnail_url")]
    urls.extend(link[2] for link in result.get("platform_links", []))
    for track in result.get("top_tracks", []):
        urls.extend(link[2] for link in track.get("links", []))
    for url in set(url for url in urls if url is not None):
        guard._validate_source_request(url)
