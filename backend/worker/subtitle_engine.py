"""Bounded standalone subtitle retrieval through yt-dlp's guarded HTTP stack."""
from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Callable

from failures import MediaFailure

# Text is decoded for validation, so bound it independently of the media-file budget.
MAX_SUBTITLE_BYTES = 4 * 1024 * 1024
LANGUAGE = re.compile(r"[a-z]{2,3}(?:-[A-Za-z0-9]{2,8})?", re.ASCII)
TIMING = re.compile(r"^(?P<start>(?:\d{2,}:)?[0-5]\d:[0-5]\d[.,]\d{3}) --> (?P<end>(?:\d{2,}:)?[0-5]\d:[0-5]\d[.,]\d{3})(?:\s.*)?$", re.ASCII)
ASS_TIME = re.compile(r"^(\d{1,9}):([0-5]\d):([0-5]\d)\.(\d{2})$", re.ASCII)


def validate_options(options: Any) -> None:
    if (not isinstance(options, dict) or set(options) != {"language", "automatic"}
            or not isinstance(options["language"], str) or len(options["language"]) > 16
            or LANGUAGE.fullmatch(options["language"]) is None
            or type(options["automatic"]) is not bool):
        raise ValueError("invalid subtitle options")


def validate_text(data: bytes, extension: str) -> None:
    try:
        text = data.decode("utf-8-sig")
    except UnicodeError as exc:
        raise MediaFailure("invalid_media") from exc
    if not text or any(ord(char) < 32 and char not in "\r\n\t" for char in text):
        raise MediaFailure("invalid_media")
    lines = text.splitlines()
    if extension == "vtt":
        if not (lines[0] == "WEBVTT" or lines[0].startswith("WEBVTT ") or lines[0].startswith("WEBVTT\t")):
            raise MediaFailure("invalid_media")
    elif extension == "srt":
        if len(lines) < 3 or not lines[0].isascii() or not lines[0].isdigit() or TIMING.fullmatch(lines[1]) is None:
            raise MediaFailure("invalid_media")
    elif extension in ("ass", "ssa"):
        _validate_ass(text)
        return
    else:
        raise MediaFailure("unsupported_source")
    def milliseconds(value: str) -> int:
        parts = value.replace(",", ".").split(":")
        seconds, fraction = parts[-1].split(".")
        return ((int(parts[-3]) * 3600 if len(parts) == 3 else 0) + int(parts[-2]) * 60 + int(seconds)) * 1000 + int(fraction)
    timings = [match for line in lines if (match := TIMING.fullmatch(line))]
    separator = "," if extension == "srt" else "."
    if (not timings or any(separator not in m["start"] or separator not in m["end"]
            or extension == "srt" and (m["start"].count(":") != 2 or m["end"].count(":") != 2)
            or milliseconds(m["end"]) <= milliseconds(m["start"]) for m in timings)):
        raise MediaFailure("invalid_media")


def _validate_ass(text: str) -> None:
    """Validate the event structure without interpreting or rendering ASS tags."""
    if text.splitlines()[0] != "[Script Info]":
        raise MediaFailure("invalid_media")
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(";"):
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            current = stripped[1:-1].strip().casefold()
            sections.setdefault(current, [])
        elif current is not None:
            sections[current].append(stripped)

    if "script info" not in sections or "events" not in sections:
        raise MediaFailure("invalid_media")

    fields: list[str] | None = None
    dialogue_count = 0
    for line in sections["events"]:
        key, separator, value = line.partition(":")
        if not separator:
            continue
        key = key.strip().casefold()
        if key == "format":
            if fields is not None or dialogue_count:
                raise MediaFailure("invalid_media")
            fields = [field.strip().casefold() for field in value.split(",")]
            if (not all(fields) or len(set(fields)) != len(fields)
                    or fields[-1] != "text" or not {"start", "end"}.issubset(fields)):
                raise MediaFailure("invalid_media")
            continue
        if key != "dialogue":
            continue
        if fields is None:
            raise MediaFailure("invalid_media")
        values = value.lstrip().split(",", len(fields) - 1)
        if len(values) != len(fields):
            raise MediaFailure("invalid_media")
        row = dict(zip(fields, values))
        start, end = ASS_TIME.fullmatch(row["start"].strip()), ASS_TIME.fullmatch(row["end"].strip())
        if start is None or end is None or not row["text"].strip():
            raise MediaFailure("invalid_media")
        def to_centiseconds(match: re.Match[str]) -> int:
            return ((int(match[1]) * 60 + int(match[2])) * 60 + int(match[3])) * 100 + int(match[4])
        if to_centiseconds(end) <= to_centiseconds(start):
            raise MediaFailure("invalid_media")
        dialogue_count += 1
    if not dialogue_count:
        raise MediaFailure("invalid_media")


def download_subtitles(request: dict[str, Any], base_options: dict[str, Any], workspace: Path,
                       max_bytes: int, max_total: int, max_items: int,
                       progress: Callable[[float], None]) -> list[dict[str, Any]]:
    import yt_dlp
    from yt_dlp.networking import Request

    selection = (request.get("options") or {}).get("subtitles")
    try:
        validate_options(selection)
    except ValueError as exc:
        raise MediaFailure("invalid_media") from exc
    language = selection["language"]
    languages = [language] if language == "en" else [language, language + "-orig"]
    opts = {**base_options, "skip_download": True, "writesubtitles": True,
            "writeautomaticsub": selection["automatic"],
            "subtitleslangs": [rf"^{re.escape(code)}$" for code in languages],
            "subtitlesformat": "srt/vtt/ass/ssa", "format": "best", "ignore_no_formats_error": True,
            "noplaylist": True, "playlist_items": "1", "lazy_playlist": True,
            "match_filter": None, "postprocessors": []}
    result = []
    validated = []
    total = 0
    limit = min(max_bytes, MAX_SUBTITLE_BYTES)
    with yt_dlp.YoutubeDL(opts) as downloader:
        info = downloader.extract_info(request["source_url"], download=False)
        if not isinstance(info, dict) or info.get("_type", "video") != "video":
            raise MediaFailure("unsupported_source")
        selected = info.get("requested_subtitles") or {}
        if not selected:
            raise MediaFailure("unsupported_source")
        if len(selected) > min(max_items, len(languages)):
            raise MediaFailure("resource_limit")
        for index, (code, subtitle) in enumerate(sorted(selected.items())):
            if code not in languages or subtitle.get("ext") not in ("srt", "vtt", "ass", "ssa"):
                raise MediaFailure("unsupported_source")
            extension = subtitle["ext"]
            path = workspace / f"subtitles-{code}.{extension}"
            data = bytearray()
            if subtitle.get("data") is not None:
                inline = subtitle["data"]
                if not isinstance(inline, str) or len(inline) > limit:
                    raise MediaFailure("resource_limit")
                data.extend(inline.encode("utf-8"))
                if len(data) > limit or total + len(data) > max_total:
                    raise MediaFailure("resource_limit")
            else:
                headers = {**(info.get("http_headers") or {}), **(subtitle.get("http_headers") or {})}
                with downloader.urlopen(Request(subtitle["url"], headers=headers)) as response:
                    size = response.headers.get("Content-Length")
                    if size is not None and (int(size) > limit or total + int(size) > max_total):
                        raise MediaFailure("resource_limit")
                    while chunk := response.read(min(8192, limit - len(data) + 1)):
                        if len(data) + len(chunk) > limit or total + len(data) + len(chunk) > max_total:
                            raise MediaFailure("resource_limit")
                        data.extend(chunk)
            validate_text(data, extension)
            validated.append((path, data, extension))
            total += len(data)
            progress(75.0 + 25.0 * (index + 1) / len(selected))
        # Do not leave any output artifacts when a later track fails validation.
        for path, data, extension in validated:
            path.write_bytes(data)
            media_type = ("text/vtt" if extension == "vtt" else
                          "text/x-ssa" if extension in ("ass", "ssa") else "application/x-subrip")
            result.append({"path": path.name, "size_bytes": len(data), "media_type": media_type})
    return result
