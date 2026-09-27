"""One-request NDJSON media worker. All diagnostics are deliberately redacted."""
from __future__ import annotations

import json
import logging
import os
import re
import selectors
import stat
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit
import warnings

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

from network_guard import install_guard, validate_source_url

MAX_FRAME_BYTES = 64 * 1024
MAX_CONFIG_BYTES = 50 * 1024 * 1024
PROTOCOL_VERSION = 1
_UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)


@dataclass(frozen=True)
class Settings:
    max_bytes: int
    socket_timeout_secs: int
    allowed_private_origins: tuple[str, ...]

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "Settings":
        env = os.environ if env is None else env
        max_bytes = _bounded_integer(env.get("YOINK_WORKER_MAX_BYTES", str(MAX_CONFIG_BYTES)), 1, MAX_CONFIG_BYTES)
        timeout = _bounded_integer(env.get("YOINK_WORKER_SOCKET_TIMEOUT_SECS", "30"), 1, 300)
        try:
            raw_origins = json.loads(env.get("YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS", "[]"))
        except (TypeError, json.JSONDecodeError) as exc:
            raise ValueError("invalid settings") from exc
        if not isinstance(raw_origins, list) or any(not isinstance(item, str) for item in raw_origins):
            raise ValueError("invalid settings")
        from network_guard import _origin_host_port
        origins = tuple(raw_origins)
        for origin in origins:
            _origin_host_port(origin)
        return cls(max_bytes, timeout, origins)


def _bounded_integer(raw: str, minimum: int, maximum: int) -> int:
    try:
        value = int(raw)
    except (TypeError, ValueError) as exc:
        raise ValueError("invalid settings") from exc
    if not minimum <= value <= maximum:
        raise ValueError("invalid settings")
    return value


def _emit(payload: dict[str, Any]) -> None:
    line = json.dumps(payload, separators=(",", ":"), ensure_ascii=True)
    if len(line.encode("utf-8")) > MAX_FRAME_BYTES:
        raise RuntimeError("event too large")
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


def _event_prefix(request: dict[str, Any]) -> dict[str, Any]:
    return {"version": PROTOCOL_VERSION, "operation_id": request["operation_id"], "attempt": request["attempt"]}


def _progress(request: dict[str, Any], percent: float) -> None:
    _emit({**_event_prefix(request), "event": {"type": "progress", "percent": max(0.0, min(99.0, percent)), "stage": "fetching"}})


def _failure(request: dict[str, Any], failure_class: str) -> None:
    _emit({**_event_prefix(request), "event": {"type": "failure", "class": failure_class}})


def _validate_request(raw: bytes) -> dict[str, Any]:
    if len(raw) > MAX_FRAME_BYTES:
        raise ValueError("invalid request")
    request = json.loads(raw)
    if not isinstance(request, dict) or set(request) != {"version", "operation_id", "attempt", "source_url", "task"}:
        raise ValueError("invalid request")
    if type(request["version"]) is not int or request["version"] != 1:
        raise ValueError("invalid request")
    if not isinstance(request["operation_id"], str) or not _UUID_RE.fullmatch(request["operation_id"]) or request["operation_id"].lower() == "00000000-0000-0000-0000-000000000000":
        raise ValueError("invalid request")
    if not isinstance(request["attempt"], int) or isinstance(request["attempt"], bool) or request["attempt"] <= 0 or request["attempt"] > 0xFFFFFFFF:
        raise ValueError("invalid request")
    source_url = request["source_url"]
    if not isinstance(source_url, str):
        raise ValueError("invalid request")
    parsed = urlsplit(source_url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username is not None or parsed.password is not None:
        raise ValueError("invalid request")
    try:
        parsed.port
    except ValueError as exc:
        raise ValueError("invalid request") from exc
    task = request["task"]
    if not isinstance(task, dict) or task.get("kind") not in ("download", "extract_content"):
        raise ValueError("invalid request")
    if task["kind"] == "download":
        if set(task) != {"kind", "media_mode"} or task["media_mode"] not in ("video", "best", "audio"):
            raise ValueError("invalid request")
    elif set(task) != {"kind"}:
        raise ValueError("invalid request")
    return request


def _check_budget(downloaded: int, total: int | None, max_bytes: int) -> None:
    if downloaded > max_bytes or (total is not None and total > max_bytes):
        raise _ResourceLimit("resource limit exceeded")


def _classify_error(exc: BaseException) -> str:
    chain: list[BaseException] = []
    current: BaseException | None = exc
    while current is not None and current not in chain:
        chain.append(current)
        current = current.__cause__ or current.__context__
    messages = " ".join(str(item).lower() for item in chain)
    if any("limit" in type(item).__name__.lower() for item in chain) or "budget" in messages or "too large" in messages or "resource limit" in messages:
        return "resource_limit"
    if "unsupported" in messages or "not a video" in messages or "no suitable" in messages:
        return "unsupported_source"

    statuses = {getattr(item, "code", None) for item in chain}
    statuses.update(int(match) for match in re.findall(r"http error\s+(\d{3})\b", messages))
    if 429 in statuses or "rate limit" in messages:
        return "rate_limited"
    if statuses.intersection({408, 500, 502, 503, 504}):
        return "source_unavailable"
    transient_words = ("timed out", "timeout", "connection reset", "connection aborted", "remote end closed connection")
    if any(isinstance(item, (OSError, TimeoutError)) for item in chain) or any(word in messages for word in transient_words) or "network destination" in messages:
        return "source_unavailable"
    if "ffprobe" in messages or "processing" in messages:
        return "processing_failed"
    return "invalid_media"


def _verify_video(path: Path, timeout: int, _ffprobe: str = "ffprobe") -> bool:
    command = [
        _ffprobe, "-v", "error", "-protocol_whitelist", "file,pipe", "-show_entries",
        "stream=codec_type", "-of", "json", str(path),
    ]
    process: subprocess.Popen[bytes] | None = None
    try:
        deadline = time.monotonic() + timeout
        process = subprocess.Popen(
            command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, shell=False,
        )
        assert process.stdout is not None
        output = bytearray()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                if not selector.select(remaining):
                    return False
                read_size = min(4096, 64 * 1024 + 1 - len(output))
                chunk = os.read(process.stdout.fileno(), read_size)
                if not chunk:
                    break
                output.extend(chunk)
                if len(output) > 64 * 1024:
                    return False
        remaining = deadline - time.monotonic()
        if remaining <= 0 or process.wait(timeout=remaining) != 0:
            return False
        try:
            data = json.loads(output)
        except (json.JSONDecodeError, UnicodeDecodeError):
            return False
        return any(item.get("codec_type") == "video" for item in data.get("streams", []))
    except (OSError, subprocess.SubprocessError, ValueError):
        return False
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.wait()
            if process.stdout is not None:
                process.stdout.close()


def _download(request: dict[str, Any], settings: Settings) -> None:
    task = request["task"]
    if task["kind"] != "download" or task["media_mode"] == "audio":
        _failure(request, "unsupported_source")
        return
    try:
        install_guard(settings.allowed_private_origins)
        validate_source_url(request["source_url"], settings.allowed_private_origins)
        import yt_dlp
        warnings.filterwarnings("ignore")
        class SilentLogger:
            def debug(self, _message): pass
            def info(self, _message): pass
            def warning(self, _message): pass
            def error(self, _message): pass
        downloaded = 0

        def progress_hook(status: dict[str, Any]) -> None:
            nonlocal downloaded
            downloaded = max(downloaded, int(status.get("downloaded_bytes") or 0))
            total = status.get("total_bytes") or status.get("total_bytes_estimate")
            _check_budget(downloaded, int(total) if total is not None else None, settings.max_bytes)
            if status.get("status") == "downloading":
                percent = (downloaded * 100.0 / int(total)) if total else 0.0
                _progress(request, percent)

        options = {
            "outtmpl": "media.%(ext)s",
            "noplaylist": True,
            "retries": 0,
            "fragment_retries": 0,
            "socket_timeout": settings.socket_timeout_secs,
            "format": "best[protocol=https]/best[protocol=http]",
            "logger": SilentLogger(),
            "quiet": True,
            "no_warnings": True,
            "ignoreconfig": True,
            "plugin_dirs": [],
            "cookiefile": None,
            "cookiesfrombrowser": None,
            "proxy": "",
            "external_downloader": None,
            "impersonate": None,
            "progress_hooks": [progress_hook],
            "overwrites": True,
            "continuedl": False,
            "cachedir": False,
            "extract_flat": False,
        }
        with yt_dlp.YoutubeDL(options) as ydl:
            ydl.download([request["source_url"]])
    except _ResourceLimit:
        _failure(request, "resource_limit")
        return
    except BaseException as exc:
        _failure(request, _classify_error(exc))
        return

    try:
        cwd = Path.cwd()
        entries = list(cwd.iterdir())
        files = [entry for entry in entries if entry.name.startswith("media.")]
        if len(files) != 1 or len(entries) != 1:
            raise ValueError("invalid output")
        artifact = files[0]
        info = artifact.lstat()
        if not stat.S_ISREG(info.st_mode) or artifact.is_symlink():
            raise ValueError("invalid output")
        if artifact.suffix not in (".mp4", ".webm") or info.st_size > settings.max_bytes:
            raise _ResourceLimit()
        if info.st_size <= 0 or not _verify_video(artifact, settings.socket_timeout_secs):
            raise ValueError("invalid video")
        media_type = "video/mp4" if artifact.suffix == ".mp4" else "video/webm"
        _progress(request, 100.0)
        _emit({**_event_prefix(request), "event": {"type": "completed", "artifacts": [
            {"path": artifact.name, "size_bytes": info.st_size, "media_type": media_type}
        ]}})
    except BaseException as exc:
        _failure(request, _classify_error(exc))


class _ResourceLimit(Exception):
    pass


def main() -> int:
    raw = sys.stdin.buffer.read(MAX_FRAME_BYTES + 1)
    try:
        request = _validate_request(raw)
        settings = Settings.from_env()
    except BaseException:
        return 2
    _download(request, settings)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
