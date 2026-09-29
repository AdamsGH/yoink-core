"""Metadata-only, bounded source format reports."""
from __future__ import annotations

from pathlib import Path
from typing import Any

from failures import MediaFailure

MAX_REPORT_BYTES = 64 * 1024
HEADER = "Yoink source formats v1\nID\tEXT\tWIDTH\tHEIGHT\tFPS\tVIDEO\tAUDIO\tPROTOCOL\n"
EXTENSIONS = {"mp4", "webm", "m4a", "mp3", "ogg", "opus", "flv", "mkv"}
VIDEO_CODECS = {"h264": "h264", "avc1": "h264", "h265": "h265", "hevc": "h265",
                "av01": "av1", "av1": "av1", "vp09": "vp9", "vp9": "vp9", "vp08": "vp8", "vp8": "vp8"}
AUDIO_CODECS = {"aac": "aac", "mp4a": "aac", "mp3": "mp3", "opus": "opus",
                "vorbis": "vorbis", "flac": "flac"}


def validate_options(options: Any) -> None:
    if (options.get("max_height") is not None or options.get("video_codec") != "h264"
            or options.get("audio_codec") != "best" or options.get("container") != "mp4"
            or options.get("engine") not in ("auto", "yt_dlp") or options.get("playlist") is not None
            or options.get("clips") != [] or options.get("gallery_zip") is not False
            or options.get("subtitles") is not None):
        raise ValueError("invalid format options")


def _codec(value: Any, mapping: dict[str, str]) -> str:
    if not isinstance(value, str):
        return "unknown"
    token = value.split(".", 1)[0].split("(", 1)[0].strip().lower()
    return mapping.get(token, "unknown")


def _dimension(value: Any) -> str:
    return str(value) if type(value) is int and 0 <= value <= 16384 else "-"


def _fps(value: Any) -> str:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 <= value <= 1000:
        return "-"
    if value == 0:
        return "0"
    rendered = f"{value:.3f}".rstrip("0").rstrip(".")
    return rendered or "0"


def _row(fmt: Any) -> str:
    if not isinstance(fmt, dict):
        raise MediaFailure("invalid_media")
    ident = fmt.get("format_id")
    ident = ident if isinstance(ident, str) and ident.isascii() and ident.isdigit() and len(ident) <= 10 else "-"
    ext_value = fmt.get("ext")
    ext = ext_value.lower() if isinstance(ext_value, str) and ext_value.lower() in EXTENSIONS else "unknown"
    video_value = fmt.get("vcodec")
    audio_value = fmt.get("acodec")
    video = "none" if video_value == "none" else _codec(video_value, VIDEO_CODECS)
    audio = "none" if audio_value == "none" else _codec(audio_value, AUDIO_CODECS)
    protocol_value = fmt.get("protocol")
    protocol = "unknown"
    if isinstance(protocol_value, str):
        value = protocol_value.lower()
        protocol = ("hls" if "m3u8" in value or value == "hls" else
                    "dash" if "dash" in value else "https" if value.startswith("https")
                    else "http" if value.startswith("http") else "unknown")
    cells = [ident, ext, _dimension(fmt.get("width")), _dimension(fmt.get("height")),
             _fps(fmt.get("fps")), video, audio, protocol]
    return "\t".join(cells)


def download_format_report(request: dict[str, Any], base_options: dict[str, Any], workspace: Path,
                           max_bytes: int, max_total: int, max_items: int,
                           progress) -> list[dict[str, Any]]:
    del max_items
    import yt_dlp

    if (max_bytes < 1 or max_total < 1 or min(max_bytes, max_total) < len(HEADER.encode("ascii"))):
        raise MediaFailure("resource_limit")
    options = request.get("options") or {}
    try:
        validate_options({"max_height": options.get("max_height"), "video_codec": options.get("video_codec", "h264"),
                          "audio_codec": options.get("audio_codec", "best"), "container": options.get("container", "mp4"),
                          "engine": options.get("engine", "auto"), "playlist": options.get("playlist"),
                          "clips": options.get("clips", []), "gallery_zip": options.get("gallery_zip", False),
                          "subtitles": options.get("subtitles")})
    except ValueError:
        raise MediaFailure("invalid_media")
    opts = {**base_options, "skip_download": True, "noplaylist": True,
            "playlist_items": "1", "lazy_playlist": True, "extract_flat": "in_playlist",
            "format": None, "match_filter": None, "postprocessors": [],
            "writesubtitles": False, "writeautomaticsub": False, "listsubtitles": False,
            "progress_hooks": [], "post_hooks": [], "simulate": True}
    try:
        with yt_dlp.YoutubeDL(opts) as downloader:
            info = downloader.extract_info(request["source_url"], download=False)
    except MediaFailure:
        raise
    except BaseException as exc:
        from failures import classify_error
        raise MediaFailure(classify_error(exc)) from exc
    if (not isinstance(info, dict) or info.get("_type", "video") != "video"
            or info.get("is_live") is True or info.get("live_status") in ("is_live", "is_upcoming")):
        raise MediaFailure("unsupported_source")
    formats = info.get("formats")
    if not isinstance(formats, list) or not formats:
        raise MediaFailure("unsupported_source")
    if any(not isinstance(fmt, dict) for fmt in formats):
        raise MediaFailure("invalid_media")
    eligible = [fmt for fmt in formats if not fmt.get("has_drm") and not fmt.get("drm_family")
                and not (fmt.get("vcodec") == "none" and fmt.get("acodec") == "none")
                and (any(isinstance(fmt.get(key), str) and fmt[key] != "none" for key in ("vcodec", "acodec"))
                     or (isinstance(fmt.get("ext"), str) and fmt["ext"].lower() in EXTENSIONS))]
    if not eligible:
        raise MediaFailure("unsupported_source")
    if len(eligible) > 100:
        raise MediaFailure("resource_limit")
    try:
        body = HEADER + "".join(_row(fmt) + "\n" for fmt in eligible)
        data = body.encode("ascii")
    except (UnicodeError, ValueError, OverflowError) as exc:
        raise MediaFailure("invalid_media") from exc
    if len(data) > min(MAX_REPORT_BYTES, max_bytes, max_total):
        raise MediaFailure("resource_limit")
    path = workspace / "formats.txt"
    path.write_bytes(data)
    progress(100.0)
    return [{"path": path.name, "size_bytes": len(data), "media_type": "text/plain"}]
