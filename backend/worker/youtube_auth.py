"""Operator-only Unix HTTP adapter. No TCP origin exemption, redirects or remote paths."""
import http.client
import json
import os
import socket
import time
from datetime import datetime, timezone, timedelta
from pathlib import Path
from urllib.parse import urlsplit
from failures import MediaFailure

SOCKET_ENV = "YOINK_YOUTUBE_AUTH_SOCKET"


def validate_tokens(value):
    if not isinstance(value, dict) or set(value) != {"access_token", "refresh_token", "expires_at"}:
        raise ValueError("invalid OAuth credential")
    if len(("__oauth2__" + json.dumps(value, ensure_ascii=False)).encode()) > 65536:
        raise ValueError("invalid OAuth credential")
    for key in ("access_token", "refresh_token"):
        text = value[key]
        if not isinstance(text, str) or not text or len(text.encode()) > 16384 or any(ord(c) < 32 or ord(c) == 127 for c in text):
            raise ValueError("invalid OAuth credential")
    expiry = value["expires_at"]
    if not isinstance(expiry, str) or len(expiry) > 64:
        raise ValueError("invalid OAuth credential")
    parsed = datetime.fromisoformat(expiry.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("invalid OAuth credential")
    return parsed


class UnixHTTP(http.client.HTTPConnection):
    def connect(self):
        path = os.environ.get(SOCKET_ENV, "")
        # Fixed operator configuration, not accepted in any request/settings override.
        if not path.startswith("/") or len(path.encode()) > 100:
            raise MediaFailure("source_unavailable")
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(path)


def call(endpoint, payload, timeout, limit, output=None):
    connection = UnixHTTP("localhost", timeout=timeout)
    deadline = time.monotonic() + timeout
    try:
        body = json.dumps(payload, ensure_ascii=False).encode()
        if len(body) > 64 * 1024:
            raise MediaFailure("resource_limit")
        connection.request("POST", endpoint, body, {"Content-Type": "application/json"})
        wire_socket = connection.sock
        response = connection.getresponse()
        if response.status != 200 or response.getheader("x-updated-tokens"):
            # Refresh is management-owned. Refreshed execution cannot deliver old versions.
            raise MediaFailure("source_unavailable")
        length = response.getheader("Content-Length")
        if length and int(length) > limit:
            raise MediaFailure("resource_limit")
        data = bytearray()
        count = 0
        while True:
            if length and count == int(length):
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise MediaFailure("source_unavailable")
            wire_socket.settimeout(remaining)
            chunk = response.read1(min(65536, limit + 1 - count))
            if not chunk:
                break
            count += len(chunk)
            if count > limit:
                raise MediaFailure("resource_limit")
            if output is None:
                data.extend(chunk)
            else:
                output.write(chunk)
        if length and count != int(length):
            raise MediaFailure("invalid_media")
        if not count:
            raise MediaFailure("invalid_media")
        return bytes(data)
    except MediaFailure:
        raise
    except Exception:
        # Service bodies/exceptions may contain secrets; never attach their diagnostic text.
        raise MediaFailure("source_unavailable") from None
    finally:
        connection.close()


def download(request, settings, workspace, metadata):
    tokens = request["network"]["youtube_oauth"]
    if validate_tokens(tokens) <= datetime.now(timezone.utc) + timedelta(seconds=30):
        raise MediaFailure("source_unavailable")
    parsed = urlsplit(request["source_url"])
    if parsed.hostname not in ("youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be"):
        raise MediaFailure("unsupported_source")
    options = request.get("options") or {}
    mode = request["task"]["media_mode"]
    # Sidecar supports individual video/audio, not playlists/subtitles/format reports.
    if mode not in ("best", "video", "audio") or options.get("playlist") or request["network"].get("proxy_url"):
        raise MediaFailure("unsupported_source")
    payload = {"url": request["source_url"], "tokens": {
        "access_token": tokens["access_token"], "refresh_token": tokens["refresh_token"],
        "expiry_date": tokens["expires_at"]}}
    info = json.loads(call("/info", payload, settings.socket_timeout_secs, 64 * 1024))
    # Explicit allow-list: tokens/service fields cannot reach artifacts or API metadata.
    metadata.add({k: info[k] for k in ("title", "description", "tags", "categories") if k in info})
    payload.update(quality=str(options["max_height"]) if options.get("max_height") else "best", audio_only=mode == "audio")
    target = workspace / ("source-oauth.m4a" if mode == "audio" else "source-oauth.mp4")
    try:
        with target.open("xb") as output:
            call("/download", payload, settings.processing_timeout_secs,
                 min(settings.max_bytes, settings.total_max_bytes), output)
    except BaseException:
        target.unlink(missing_ok=True)
        raise
    return target
