"""One-request NDJSON worker. Credentials enter on stdin, never in diagnostics."""
from __future__ import annotations

import json
import logging
import math
import os
import re
import sys
import unicodedata
import warnings
from dataclasses import dataclass
from typing import Any

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

# Isolated Python excludes the script directory. Add only the immutable worker
# source directory, never the operation workspace or caller-controlled PYTHONPATH.
sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))

from failures import classify_error as _classify_error
from format_engine import validate_options as validate_format_options
from media_engine import MediaFailure, download_media
from network_guard import configure_proxy, guard_context, validate_source_url
from proxy_tunnel import proxy_route
from subtitle_engine import validate_options as validate_subtitle_options

MAX_FRAME_BYTES = 1024 * 1024
MAX_CONFIG_BYTES = 50 * 1024 * 1024
MAX_ARTIFACT_BYTES = 2 * 1024 ** 3
MAX_TOTAL_BYTES = 8 * 1024 ** 3
PROTOCOL_VERSION = 2
_UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)


@dataclass(frozen=True)
class Settings:
    max_bytes: int
    total_max_bytes: int
    max_items: int
    socket_timeout_secs: int
    allowed_private_origins: tuple[str, ...]
    allowed_proxy_origins: tuple[str, ...]
    processing_timeout_secs: int = 300

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "Settings":
        env = os.environ if env is None else env
        max_bytes = _bounded_integer(env.get("YOINK_WORKER_MAX_BYTES", str(MAX_CONFIG_BYTES)), 1, MAX_ARTIFACT_BYTES)
        total_bytes = _bounded_integer(env.get("YOINK_WORKER_TOTAL_MAX_BYTES", str(MAX_CONFIG_BYTES)), 1, MAX_TOTAL_BYTES)
        max_items = _bounded_integer(env.get("YOINK_WORKER_MAX_ITEMS", "20"), 1, 100)
        timeout = _bounded_integer(env.get("YOINK_WORKER_SOCKET_TIMEOUT_SECS", "30"), 1, 300)
        processing = _bounded_integer(env.get("YOINK_WORKER_PROCESSING_TIMEOUT_SECS", "300"), 1, 86400)
        try:
            raw_origins = json.loads(env.get("YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS", "[]"))
            raw_proxies = json.loads(env.get("YOINK_WORKER_ALLOWED_PROXY_ORIGINS", "[]"))
        except (TypeError, json.JSONDecodeError) as exc:
            raise ValueError("invalid settings") from exc
        if any(not isinstance(values, list) or any(not isinstance(item, str) for item in values) for values in (raw_origins, raw_proxies)):
            raise ValueError("invalid settings")
        from network_guard import _origin_host_port
        origins = tuple(raw_origins)
        proxy_origins = tuple(raw_proxies)
        for origin in origins + proxy_origins:
            _origin_host_port(origin)
        return cls(max_bytes, total_bytes, max_items, timeout, origins, proxy_origins, processing)


def _bounded_integer(raw: str, minimum: int, maximum: int) -> int:
    try:
        value = int(raw)
    except (TypeError, ValueError) as exc:
        raise ValueError("invalid settings") from exc
    if not minimum <= value <= maximum:
        raise ValueError("invalid settings")
    return value


def _emit(payload: dict[str, Any]) -> None:
    line = json.dumps(payload, separators=(",", ":"), ensure_ascii=True, allow_nan=False)
    if len(line.encode("utf-8")) > MAX_FRAME_BYTES:
        raise RuntimeError("event too large")
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


def _event_prefix(request: dict[str, Any]) -> dict[str, Any]:
    return {"version": PROTOCOL_VERSION, "operation_id": request["operation_id"], "attempt": request["attempt"]}


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("invalid request")
        result[key] = value
    return result


def _failure(request: dict[str, Any], failure_class: str) -> None:
    _emit({**_event_prefix(request), "event": {"type": "failure", "class": failure_class}})


def _validate_request(raw: bytes) -> dict[str, Any]:
    if len(raw) > MAX_FRAME_BYTES:
        raise ValueError("invalid request")
    def reject_constant(_value: str) -> Any:
        raise ValueError("invalid request")
    request = json.loads(raw, parse_constant=reject_constant, object_pairs_hook=_unique_object)
    if isinstance(request, dict) and isinstance(request.get("task"), dict) and request["task"].get("kind", "").startswith("music_"):
        from music_engine import validate
        return validate(request)
    if isinstance(request, dict) and isinstance(request.get("task"), dict) and request["task"].get("kind") == "full_content_fetch":
        from full_content import validate
        return validate(request)
    if isinstance(request, dict) and isinstance(request.get("task"), dict) and request["task"].get("kind") in ("content_get", "content_readability", "content_trafilatura"):
        from content_stage import validate
        return validate(request)
    if isinstance(request, dict) and isinstance(request.get("task"), dict) and request["task"].get("kind") == "youtube_search":
        return _validate_search_request(request, len(raw))
    required = {"version", "operation_id", "attempt", "source_url", "task"}
    if not isinstance(request, dict) or not required.issubset(request) or set(request) - required - {"options", "network"}:
        raise ValueError("invalid request")
    if type(request["version"]) is not int or request["version"] != 2:
        raise ValueError("invalid request")
    if not isinstance(request["operation_id"], str) or not _UUID_RE.fullmatch(request["operation_id"]) or request["operation_id"].lower() == "00000000-0000-0000-0000-000000000000":
        raise ValueError("invalid request")
    if not isinstance(request["attempt"], int) or isinstance(request["attempt"], bool) or request["attempt"] <= 0 or request["attempt"] > 0xFFFFFFFF:
        raise ValueError("invalid request")
    source_url = request["source_url"]
    if not isinstance(source_url, str):
        raise ValueError("invalid request")
    from network_guard import _parse_http_url
    _parse_http_url(source_url, "invalid request")
    task = request["task"]
    if not isinstance(task, dict) or task.get("kind") not in ("download", "extract_content"):
        raise ValueError("invalid request")
    if task["kind"] == "download":
        if set(task) != {"kind", "media_mode"} or task["media_mode"] not in ("video", "best", "audio", "image", "subtitles", "formats", "links", "audio_links"):
            raise ValueError("invalid request")
    elif set(task) != {"kind"}:
        raise ValueError("invalid request")
    options = request.get("options", {})
    option_keys = {"max_height", "video_codec", "audio_codec", "container", "engine", "playlist", "clips", "gallery_zip", "subtitles", "media_info"}
    if not isinstance(options, dict) or set(options) - option_keys:
        raise ValueError("invalid request")
    defaults = {"max_height": None, "video_codec": "h264", "audio_codec": "best", "container": "mp4", "engine": "auto", "playlist": None, "clips": [], "gallery_zip": False, "media_info": False}
    defaults.update(options)
    if defaults["max_height"] is not None and (type(defaults["max_height"]) is not int or defaults["max_height"] <= 0 or defaults["max_height"] > 100000):
        raise ValueError("invalid request")
    if defaults["video_codec"] not in ("h264", "h265", "av1", "vp9", "any") or defaults["audio_codec"] not in ("best", "aac", "mp3", "opus") or defaults["container"] not in ("mp4", "mkv", "webm") or defaults["engine"] not in ("auto", "yt_dlp", "gallery", "auto_yt_dlp_v1", "auto_gallery_v1", "auto_fallback_v1") or type(defaults["gallery_zip"]) is not bool:
        raise ValueError("invalid request")
    if type(defaults["media_info"]) is not bool:
        raise ValueError("invalid request")
    playlist = defaults["playlist"]
    if playlist is not None:
        if not isinstance(playlist, dict) or set(playlist) != {"start", "end"} or type(playlist["start"]) is not int or (playlist["end"] is not None and type(playlist["end"]) is not int):
            raise ValueError("invalid request")
        start, end = playlist["start"], playlist["end"]
        if not -0x80000000 <= start <= 0x7FFFFFFF or end is not None and not -0x80000000 <= end <= 0x7FFFFFFF or end is None and start < -100:
            raise ValueError("invalid request")
        if start == 0 or end == 0 or end is not None and (start * end < 0 or abs(end - start) + 1 > 100):
            raise ValueError("invalid request")
    clips = defaults["clips"]
    if not isinstance(clips, list) or len(clips) > 20:
        raise ValueError("invalid request")
    for clip in clips:
        if not isinstance(clip, dict) or set(clip) != {"start_seconds", "end_seconds"}:
            raise ValueError("invalid request")
        start, end = clip["start_seconds"], clip["end_seconds"]
        if type(start) not in (float, int) or type(end) not in (float, int) or not (0 <= start < end < float("inf")):
            raise ValueError("invalid request")
    subtitles = defaults.get("subtitles")
    if (task.get("media_mode") == "subtitles") != (subtitles is not None):
        raise ValueError("invalid request")
    if subtitles is not None:
        validate_subtitle_options(subtitles)
        if (defaults["max_height"] is not None or defaults["video_codec"] != "h264"
                or defaults["audio_codec"] != "best" or defaults["container"] != "mp4"
                or defaults["engine"] in ("gallery", "auto_gallery_v1", "auto_fallback_v1") or playlist is not None or clips or defaults["gallery_zip"]):
            raise ValueError("invalid request")
    if task.get("media_mode") in ("links", "audio_links"):
        if (defaults["max_height"] not in (None, 720, 1080, 480, 360)
                or defaults["video_codec"] != "h264" or defaults["audio_codec"] != "best"
                or defaults["container"] != "mp4" or defaults["engine"] not in ("auto", "yt_dlp", "auto_yt_dlp_v1")
                or playlist is not None or clips or defaults["gallery_zip"] or subtitles is not None
                or task["media_mode"] == "audio_links" and defaults["max_height"] is not None):
            raise ValueError("invalid request")
    if task.get("media_mode") == "formats":
        try:
            validate_format_options(defaults)
        except ValueError as exc:
            raise ValueError("invalid request") from exc
    request["options"] = defaults
    network = request.get("network")
    if network is not None:
        if not isinstance(network, dict) or set(network) - {"proxy_url", "cookie_jar", "youtube_oauth"}:
            raise ValueError("invalid request")
        proxy, cookies = network.get("proxy_url"), network.get("cookie_jar")
        oauth = network.get("youtube_oauth")
        if oauth is not None:
            from youtube_auth import validate_tokens
            validate_tokens(oauth)
            if cookies is not None:
                raise ValueError("invalid request")
        if proxy is not None:
            if not isinstance(proxy, str) or not proxy or len(proxy.encode()) > 4096 or any(ord(c) < 32 for c in proxy):
                raise ValueError("invalid request")
            proxy_parts = _parse_http_url(proxy, "invalid request", allow_credentials=True)
            if proxy_parts.path not in ("", "/") or proxy_parts.query or proxy_parts.fragment:
                raise ValueError("invalid request")
        if cookies is not None:
            if not isinstance(cookies, str) or not cookies or len(cookies.encode()) > 512 * 1024 or any(unicodedata.category(c) == "Cc" and c not in "\t\r\n" for c in cookies):
                raise ValueError("invalid request")
            for row in cookies.splitlines():
                if not row or row.startswith("#") and not row.startswith("#HttpOnly_"):
                    continue
                fields = row.split("\t")
                if len(fields) != 7 or not fields[0] or not fields[2] or fields[1] not in ("TRUE", "FALSE") or fields[3] not in ("TRUE", "FALSE") or not fields[4].isascii() or not fields[4].isdigit() or int(fields[4]) > 0xFFFFFFFFFFFFFFFF or not fields[5]:
                    raise ValueError("invalid request")
        request["network"] = {"proxy_url": proxy, "cookie_jar": cookies, "youtube_oauth": oauth}
    return request


def _validate_search_request(request, frame_bytes):
    from search_engine import validate_query, MAX_SEARCH_FRAME_BYTES
    if (frame_bytes > MAX_SEARCH_FRAME_BYTES or set(request) - {"version", "operation_id", "attempt", "task", "network"}
            or not {"version", "operation_id", "attempt", "task"}.issubset(request)
            or type(request["version"]) is not int or request["version"] != PROTOCOL_VERSION
            or not isinstance(request["operation_id"], str) or not _UUID_RE.fullmatch(request["operation_id"])
            or request["operation_id"].lower() == "00000000-0000-0000-0000-000000000000"
            or type(request["attempt"]) is not int or not 1 <= request["attempt"] <= 0xFFFFFFFF
            or set(request["task"]) != {"kind", "query"}):
        raise ValueError("search requires version, nonzero request ID/attempt and youtube_search query; source_url/options/cookies are unsupported")
    validate_query(request["task"]["query"])
    network = request.get("network")
    if network is not None:
        if not isinstance(network, dict) or set(network) - {"proxy_url"}:
            raise ValueError("search network accepts only operator proxy_url; cookies are unsupported")
        proxy = network.get("proxy_url")
        if proxy is not None:
            from network_guard import _parse_http_url
            if not isinstance(proxy, str) or not proxy or len(proxy.encode()) > 4096 or any(ord(c) < 32 for c in proxy):
                raise ValueError("invalid search proxy")
            parsed = _parse_http_url(proxy, "invalid search proxy", allow_credentials=True)
            if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
                raise ValueError("invalid search proxy")
    return request


def _search(request, settings):
    from search_engine import search
    try:
        with guard_context(settings.allowed_private_origins, settings.allowed_proxy_origins, dns_timeout=settings.socket_timeout_secs):
            network = request.get("network") or {}
            configure_proxy(network.get("proxy_url"))
            with proxy_route(network.get("proxy_url"), settings.socket_timeout_secs) as proxy:
                routed = {**request, "network": {"proxy_url": proxy}}
                entries = search(routed, settings)
        _emit({**_event_prefix(request), "event": {"type": "search_completed", "entries": entries}})
    except BaseException as exc:
        _failure(request, _classify_error(exc))


def _download(request: dict[str, Any], settings: Settings) -> None:
    last = (-1, "")

    def progress(percent: float) -> None:
        nonlocal last
        if not math.isfinite(percent):
            raise MediaFailure("invalid_media")
        value = max(last[0], min(99, max(0, int(percent))))
        stage = "fetching" if value < 75 else "processing" if value < 95 else "writing_artifact"
        if (value, stage) != last:
            _emit({**_event_prefix(request), "event": {"type": "progress", "percent": value, "stage": stage}})
            last = (value, stage)

    try:
        if request["task"]["kind"] != "download":
            raise MediaFailure("unsupported_source")
        with guard_context(settings.allowed_private_origins, settings.allowed_proxy_origins, dns_timeout=settings.socket_timeout_secs):
            network = request.get("network") or {}
            configure_proxy(network.get("proxy_url"))
            validate_source_url(request["source_url"], settings.allowed_private_origins)
            with proxy_route(network.get("proxy_url"), settings.socket_timeout_secs) as proxy:
                routed = {**request, "network": {**network, "proxy_url": proxy}}
                artifacts = download_media(routed, settings, progress)
        _emit({**_event_prefix(request), "event": {"type": "completed", "artifacts": artifacts}})
    except BaseException as exc:
        _failure(request, _classify_error(exc))


def main() -> int:
    raw = sys.stdin.buffer.read(MAX_FRAME_BYTES + 1)
    try:
        request = _validate_request(raw)
        settings = Settings.from_env()
    except BaseException:
        sys.stderr.write("invalid worker request or settings: use version 2 download/content with source_url, or youtube_search with a bounded query and no source_url/options/cookies\n")
        return 2
    if request["task"]["kind"].startswith("music_"):
        from music_engine import execute
        execute(request, settings, _emit, _event_prefix)
    elif request["task"]["kind"] == "full_content_fetch":
        from full_content import execute
        execute(request, settings, _emit, _event_prefix(request))
    elif request["task"]["kind"] in ("content_get", "content_readability", "content_trafilatura"):
        from content_stage import execute
        execute(request, settings, _emit, _event_prefix(request))
    elif request["task"]["kind"] == "youtube_search":
        _search(request, settings)
    else:
        _download(request, settings)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
