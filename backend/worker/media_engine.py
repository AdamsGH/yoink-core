"""Bounded media download and local processing for the v2 worker."""
from __future__ import annotations

import asyncio
import concurrent.futures
from contextlib import contextmanager
import json
import shutil
import os
import stat
import subprocess
import selectors
import tempfile
import time
import zipfile
from pathlib import Path
from typing import Any, Callable

import tool_guard
from content_metadata import MetadataCollector
from failures import MediaFailure, classify_error, exception_chain
from format_engine import download_format_report
from link_engine import download_links
from subtitle_engine import download_subtitles


_FORMATS = tool_guard._FORMATS
_PROTOCOLS = tool_guard._PROTOCOLS


def _bounded_output(command: list[str], timeout: int, output_limit: int = 64 * 1024) -> bytes:
    process = None
    try:
        process = subprocess.Popen(
            command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, shell=False, env={**os.environ, "LC_ALL": "C"},
        )
        assert process.stdout is not None and process.stderr is not None
        captured = {process.stdout: bytearray(), process.stderr: bytearray()}
        deadline = time.monotonic() + timeout
        with selectors.DefaultSelector() as selector:
            for stream in captured:
                selector.register(stream, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise MediaFailure("processing_failed")
                for key, _ in selector.select(remaining):
                    stream = key.fileobj
                    chunk = os.read(stream.fileno(), min(8192, output_limit + 1 - len(captured[stream])))
                    if not chunk:
                        selector.unregister(stream)
                        continue
                    captured[stream].extend(chunk)
                    if len(captured[stream]) > output_limit:
                        raise MediaFailure("processing_failed")
        remaining = deadline - time.monotonic()
        if remaining <= 0 or process.wait(timeout=remaining) != 0:
            raise MediaFailure("processing_failed")
        return bytes(captured[process.stdout])
    except MediaFailure:
        raise
    except (OSError, subprocess.SubprocessError) as exc:
        raise MediaFailure("processing_failed") from exc
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.wait()
            if process.stdout is not None:
                process.stdout.close()
            if process.stderr is not None:
                process.stderr.close()


def _tools(workspace: Path) -> tuple[str, str, Path]:
    del workspace
    wrapper_dir = Path(__file__).resolve().parent / "bin"
    ffmpeg = wrapper_dir / "ffmpeg"
    ffprobe = wrapper_dir / "ffprobe"
    if not ffmpeg.is_file() or not ffprobe.is_file():
        raise MediaFailure("processing_failed")
    return str(ffmpeg), str(ffprobe), wrapper_dir


def playlist_indices(playlist: dict[str, int | None] | None) -> list[int] | None:
    if playlist is None:
        return None
    start, end = playlist["start"], playlist["end"]
    if start == 0 or end == 0:
        raise MediaFailure("invalid_media")
    if end is None:
        if start > 0:
            return [start]
        if start < -100:
            raise MediaFailure("invalid_media")
        return list(range(start, 0))
    if (start > 0) != (end > 0):
        raise MediaFailure("invalid_media")
    step = 1 if end >= start else -1
    count = abs(end - start) + 1
    if count > 100:
        raise MediaFailure("invalid_media")
    return list(range(start, end + step, step))


def _format_expression(mode: str, options: dict[str, Any]) -> str:
    height = f"[height<={options['max_height']}]" if options.get("max_height") else ""
    container = options.get("container", "mp4")
    video = options.get("video_codec", "h264")
    audio = options.get("audio_codec", "best")
    if mode == "audio":
        if audio == "best":
            return "bestaudio/best"
        codec = {"aac": "aac", "mp3": "mp3", "opus": "opus"}[audio]
        return f"bestaudio[acodec*={codec}]/bestaudio/best"
    if mode == "image":
        return "best"
    codec_filter = {"h264": "[vcodec^=avc1]", "h265": "[vcodec^=hev]", "av1": "[vcodec^=av01]", "vp9": "[vcodec^=vp9]", "any": ""}[video]
    return f"bestvideo{height}{codec_filter}+bestaudio/best{height}/best"


def _ffmpeg_args(ffmpeg: str, source: Path, destination: Path, options: dict[str, Any],
                 mode: str, clip: dict[str, float] | None = None) -> list[str]:
    args = [ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "2", "-filter_threads", "2", "-y"]
    if clip:
        args += ["-ss", str(clip["start_seconds"]), "-i", str(source),
                 "-t", str(clip["end_seconds"] - clip["start_seconds"])]
    else:
        args += ["-i", str(source)]
    if mode == "audio":
        audio = options.get("audio_codec", "best")
        codec = {"best": "aac", "aac": "aac", "mp3": "libmp3lame", "opus": "libopus"}[audio]
        args += ["-vn", "-c:a", codec]
        if audio == "mp3" and options.get("_music_audio_bitrate") == 192:
            args += ["-b:a", "192k"]
        if audio == "mp3":
            args += ["-f", "mp3"]
        elif audio == "opus":
            args += ["-f", "opus"]
        elif audio in ("aac", "best"):
            args += ["-f", "mp4", "-movflags", "+faststart"]
    elif mode != "image":
        video = options.get("video_codec", "h264")
        codec = {"h264": "libx264", "h265": "libx265", "av1": "libaom-av1", "vp9": "libvpx-vp9", "any": "libx264" if options.get("max_height") else "copy"}[video]
        audio = options.get("audio_codec", "best")
        audio_codec = {"best": "copy", "aac": "aac", "mp3": "libmp3lame", "opus": "libopus"}[audio]
        args += ["-c:v", codec, "-c:a", audio_codec]
        if options.get("max_height"):
            args += ["-vf", f"scale=-2:min(ih\\,{int(options['max_height'])})"]
    args.append(str(destination))
    return args


def _probe(path: Path, ffprobe: str, timeout: int) -> tuple[dict[str, Any], str]:
    output = _bounded_output([
        ffprobe, "-v", "error", "-show_entries",
        "format=format_name:stream=codec_type,codec_name,width,height", "-of", "json", str(path),
    ], timeout)
    try:
        info = json.loads(output)
        streams = info.get("streams", [])
        fmt = info.get("format", {}).get("format_name", "")
    except (ValueError, AttributeError):
        raise MediaFailure("invalid_media")
    if not streams:
        raise MediaFailure("invalid_media")
    return info, fmt


def _media_type(info: dict[str, Any], format_name: str) -> str:
    streams = info.get("streams", [])
    types = {stream.get("codec_type") for stream in streams}
    codecs = {stream.get("codec_name") for stream in streams}
    formats = set(filter(None, format_name.lower().split(",")))
    if "video" in types:
        if codecs.intersection({"mjpeg", "png", "gif", "webp"}):
            codec = next(iter(codecs.intersection({"mjpeg", "png", "gif", "webp"})))
            return {"mjpeg": "image/jpeg", "png": "image/png", "gif": "image/gif", "webp": "image/webp"}[codec]
        return "video/webm" if "webm" in formats and "matroska" not in formats else "video/x-matroska" if "matroska" in formats else "video/mp4"
    if "audio" in types:
        return "audio/mpeg" if "mp3" in codecs else "audio/ogg" if "opus" in codecs and "ogg" in formats else "audio/webm" if "webm" in formats else "audio/x-matroska" if "matroska" in formats else "audio/mp4" if formats.intersection({"mov", "mp4", "m4a", "ipod"}) else "audio/aac" if "aac" in codecs else "application/octet-stream"
    return "application/octet-stream"


def _already_compatible(info: dict[str, Any], format_name: str, options: dict[str, Any]) -> bool:
    formats = set(filter(None, format_name.lower().split(",")))
    container = options.get("container", "mp4")
    expected_formats = {"mp4": {"mp4", "mov", "m4a"}, "mkv": {"matroska"}, "webm": {"webm"}}.get(container, set())
    if not expected_formats.intersection(formats):
        return False
    streams = info.get("streams", [])
    video = next((stream for stream in streams if stream.get("codec_type") == "video"), None)
    audio = next((stream for stream in streams if stream.get("codec_type") == "audio"), None)
    requested_video = options.get("video_codec", "h264")
    requested_audio = options.get("audio_codec", "best")
    video_codecs = {"h264": {"h264"}, "h265": {"hevc"}, "av1": {"av1"}, "vp9": {"vp9"}}
    audio_codecs = {"aac": {"aac"}, "mp3": {"mp3"}, "opus": {"opus"}}
    if video is None or audio is None:
        return False
    if requested_video != "any" and video.get("codec_name") not in video_codecs.get(requested_video, set()):
        return False
    if requested_audio != "best" and audio.get("codec_name") not in audio_codecs.get(requested_audio, set()):
        return False
    maximum = options.get("max_height")
    return not maximum or int(video.get("height", 0)) <= int(maximum)


def _check_size(path: Path, maximum: int) -> int:
    try:
        info = path.lstat()
    except OSError as exc:
        raise MediaFailure("invalid_media") from exc
    if not stat.S_ISREG(info.st_mode) or path.is_symlink() or info.st_size <= 0:
        raise MediaFailure("invalid_media")
    if info.st_size > maximum:
        raise MediaFailure("resource_limit")
    return info.st_size


def _bounded_transfer_write(handle: Any, data: bytes, file_size: int, total_size: int,
                           max_bytes: int, max_total: int) -> int:
    size = len(data)
    if size > max_bytes or file_size + size > max_bytes or total_size + size > max_total:
        raise MediaFailure("resource_limit")
    handle.write(data)
    return size


def _gallery_download(url: str, workspace: Path, progress: Callable[[float], None],
                      cookie_file: str | None, proxy: str, max_bytes: int,
                      max_total: int, max_items: int, timeout: int, metadata=None) -> list[Path]:
    try:
        from gallery_dl import config, job
        import builtins
        # Explicitly replace gallery-dl defaults, preventing user config and extensions.
        config.clear()
        config.set(("extractor",), "base-directory", str(workspace))
        config.set(("extractor",), "directory", [""])
        config.set(("extractor",), "filename", "{num:>04}.{extension}")
        config.set(("extractor",), "skip", True)
        config.set(("extractor",), "retries", 0)
        config.set(("extractor",), "timeout", timeout)
        config.set(("extractor",), "verify", True)
        config.set(("extractor",), "cookies", cookie_file)
        config.set(("extractor",), "cookies-update", False)
        config.set(("extractor",), "proxy", proxy)
        config.set(("output",), "mode", "null")
        config.set(("output",), "log", "null")
        # Enforce aggregate output bytes at each write, before data reaches disk.
        original_open = builtins.open
        used = 0
        files: set[str] = set()
        class BoundedWriter:
            def __init__(self, handle, path):
                self.handle, self.path = handle, path
                files.add(str(path))
                if len(files) > max_items:
                    handle.close()
                    raise MediaFailure("resource_limit")
            def write(self, data):
                nonlocal used
                size = len(data)
                try:
                    existing = max(self.path.stat().st_size, self.handle.tell())
                except OSError:
                    existing = 0
                used += _bounded_transfer_write(self.handle, data, existing, used, max_bytes, max_total)
                return size
            def __getattr__(self, name):
                return getattr(self.handle, name)
            def __enter__(self):
                self.handle.__enter__()
                return self
            def __exit__(self, *args):
                return self.handle.__exit__(*args)
        def guarded_open(file, mode="r", *args, **kwargs):
            path = Path(file) if isinstance(file, (str, bytes, os.PathLike)) else None
            writing = "w" in mode or "a" in mode or "+" in mode
            if path is not None and writing:
                try:
                    path.resolve().relative_to(workspace)
                except (ValueError, OSError) as exc:
                    raise OSError("gallery write confinement: destination is not permitted") from exc
            handle = original_open(file, mode, *args, **kwargs)
            return BoundedWriter(handle, path) if path is not None and writing else handle
        builtins.open = guarded_open
        completed = []
        ordinal = 0
        failures = []
        try:
            class StrictDownloadJob(job.DownloadJob):
                failure = None
                def handle_url(self, url, kwdict):
                    nonlocal ordinal
                    ordinal += 1
                    kwdict = {**kwdict, "num": ordinal}
                    if metadata is not None:
                        metadata.add(kwdict)
                    result = super().handle_url(url, kwdict)
                    path = Path(self.pathfmt.path)
                    if path.is_file():
                        completed.append(path)
                    return result
                def dispatch(self, extractor):
                    try:
                        return super().dispatch(extractor)
                    except MediaFailure as exc:
                        self.failure = exc
                        failures.append(exc)
                        raise
            instance = StrictDownloadJob(url)
            result = instance.run()
            if failures:
                raise failures[0]
            if result not in (None, 0) or not files:
                raise MediaFailure("source_unavailable")
        finally:
            builtins.open = original_open
    except MediaFailure:
        raise
    except Exception as exc:
        raise MediaFailure(classify_error(exc)) from exc
    progress(75.0)
    return completed


def download_media(request: dict[str, Any], settings: Any,
                   emit_progress: Callable[[float], None], *,
                   youtube_dl_factory=None) -> list[dict[str, Any]]:
    """Download and process one request in its already-private current directory."""
    workspace = Path.cwd().resolve()
    socket_timeout = int(settings.socket_timeout_secs)
    timeout = int(settings.processing_timeout_secs)
    max_bytes = int(settings.max_bytes)
    max_total = int(settings.total_max_bytes)
    max_items = int(settings.max_items)
    options = request.get("options") or {}
    task = request.get("task") or {}
    if task.get("kind") != "download":
        raise MediaFailure("unsupported_source")
    mode = task.get("media_mode")
    if mode not in ("best", "video", "audio", "image", "subtitles", "formats", "links", "audio_links"):
        raise MediaFailure("unsupported_source")
    engine = options.get("engine", "auto")
    if engine in ("gallery", "auto_gallery_v1", "auto_fallback_v1") and mode in ("formats", "subtitles", "links", "audio_links"):
        raise MediaFailure("invalid_media")
    if engine == "auto_fallback_v1":
        return _provider_fallback(request, settings, emit_progress)
    gallery = mode == "image" or engine in ("gallery", "auto_gallery_v1")
    if mode == "formats" and (
        options.get("max_height") is not None or options.get("playlist") is not None
        or options.get("clips") or options.get("gallery_zip", False)
        or options.get("video_codec", "h264") != "h264"
        or options.get("audio_codec", "best") != "best"
        or options.get("container", "mp4") != "mp4"
        or options.get("engine", "auto") not in ("auto", "yt_dlp", "auto_yt_dlp_v1")
        or options.get("subtitles") is not None
    ):
        raise MediaFailure("invalid_media")
    if mode == "image" and (
        options.get("max_height") is not None
        or options.get("video_codec", "h264") != "h264"
        or options.get("audio_codec", "best") != "best"
        or options.get("container", "mp4") != "mp4"
        or options.get("clips") or options.get("playlist")
    ):
        raise MediaFailure("invalid_media")
    if mode == "audio" and (
        options.get("max_height") is not None
        or options.get("video_codec", "h264") != "h264"
        or options.get("gallery_zip", False)
    ):
        raise MediaFailure("invalid_media")
    if mode != "image" and options.get("gallery_zip", False):
        raise MediaFailure("invalid_media")
    if mode == "image" and engine == "yt_dlp":
        raise MediaFailure("unsupported_source")
    try:
        indices = playlist_indices(options.get("playlist"))
    except (KeyError, TypeError, ValueError) as exc:
        raise MediaFailure("invalid_media") from exc
    clips = options.get("clips", [])
    from urllib.parse import urlsplit
    ranged = bool(clips) and urlsplit(request["source_url"]).hostname in (
        "youtube.com", "www.youtube.com", "music.youtube.com", "youtu.be",
    ) and indices is None
    ranged_sources = None
    if len(clips) > 20:
        raise MediaFailure("invalid_media")
    network = request.get("network") or {}
    proxy = network.get("proxy_url") or ""
    cookie_text = network.get("cookie_jar")
    cookie_file = None
    try:
        from network_guard import validate_source_url
    except ImportError as exc:
        raise MediaFailure("source_unavailable") from exc
    try:
        validate_source_url(request["source_url"], tuple(getattr(settings, "allowed_private_origins", ())))
    except OSError as exc:
        raise MediaFailure("source_unavailable") from exc
    except ValueError as exc:
        raise MediaFailure("unsupported_source") from exc
    try:
        ffmpeg, ffprobe, wrapper_dir = _tools(workspace)
        metadata = MetadataCollector()
    except MediaFailure:
        raise
    except Exception as exc:
        raise MediaFailure("processing_failed") from exc
    try:
        if cookie_text:
            if cookie_text.lstrip().startswith(("__oauth2__", "{")):
                raise MediaFailure("invalid_media")
            if not cookie_text.startswith("# Netscape"):
                cookie_text = "# Netscape HTTP Cookie File\n" + cookie_text
            fd, cookie_file = tempfile.mkstemp(prefix=".cookies-", dir=workspace)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                handle.write(cookie_text)
        if network.get("youtube_oauth") is not None:
            from youtube_auth import download as oauth_download
            oauth_download(request, settings, workspace, metadata)
        elif gallery:
            if engine == "yt_dlp":
                raise MediaFailure("unsupported_source")
            gallery_sources = _gallery_download(request["source_url"], workspace, emit_progress, cookie_file, proxy, max_bytes, max_total, max_items, socket_timeout, metadata)
        else:
            try:
                import yt_dlp
                from youtube_auth_pot import install
                install()
            except ImportError as exc:
                raise MediaFailure("unsupported_source") from exc
            class SilentLogger:
                def debug(self, _message): pass
                def info(self, _message): pass
                def warning(self, _message): pass
                def error(self, _message): pass
            downloaded_by_file: dict[str, int] = {}
            def hook(status: dict[str, Any]) -> None:
                if status.get("status") == "finished" and isinstance(status.get("info_dict"), dict):
                    metadata.add(status["info_dict"])
                filename = str(status.get("filename") or "current")
                current = int(status.get("downloaded_bytes") or 0)
                downloaded_by_file[filename] = max(downloaded_by_file.get(filename, 0), current)
                downloaded = downloaded_by_file[filename]
                estimate = status.get("total_bytes") or status.get("total_bytes_estimate")
                if (downloaded > max_bytes or sum(downloaded_by_file.values()) > max_total
                        or len({"-".join(name.split("-", 2)[:2]) for name in downloaded_by_file}) > max_items
                        or estimate and estimate > max_bytes):
                    raise MediaFailure("resource_limit")
                if status.get("status") == "downloading" and estimate:
                    emit_progress(min(70.0, downloaded * 70.0 / estimate))
            def match_filter(info, *, incomplete=False):
                if not incomplete and options.get("_music_audio_bitrate") == 192 and "Music" not in (info.get("categories") or []):
                    raise MediaFailure("unsupported_source")
                if not incomplete:
                    metadata.add(info)
                    if info.get("is_live") or info.get("live_status") in ("is_live", "post_live"):
                        raise MediaFailure("live_stream")
                known_size = info.get("filesize") or info.get("filesize_approx")
                if not ranged and known_size is not None and int(known_size) > max_bytes:
                    raise MediaFailure("resource_limit")
                return None
            index_selection = None
            if indices:
                index_selection = ",".join(str(index) for index in indices)
            opts = {
                # autonumber follows extraction order, unlike playlist_index for reverse selections.
                "outtmpl": "source-%(autonumber)05d-%(playlist_index)05d-%(id)s.%(ext)s", "noplaylist": indices is None,
                "playlist_items": index_selection, "retries": 0, "fragment_retries": 0,
                "skip_unavailable_fragments": False,
                "socket_timeout": socket_timeout, "format": _format_expression(mode, options),
                "logger": SilentLogger(), "quiet": True, "no_warnings": True,
                "ignoreconfig": True, "plugin_dirs": [], "cookiefile": cookie_file,
                "cookiesfrombrowser": None, "proxy": proxy, "external_downloader": None,
                "impersonate": None, "progress_hooks": [_local_callback(hook)], "overwrites": True,
                "continuedl": False, "cachedir": False, "extract_flat": False,
                "ffmpeg_location": str(wrapper_dir),
                "merge_output_format": options.get("container", "mp4"),
                "match_filter": _local_callback(match_filter),
                "extractor_args": {},
                "compat_opts": set(),
            }
            if options.get("max_height"):
                opts["format_sort"] = [f"res:{options['max_height']}"]
            if mode == "subtitles":
                return download_subtitles(request, opts, workspace, max_bytes, max_total, max_items, emit_progress)
            if mode in ("links", "audio_links"):
                return download_links(request, opts, workspace, max_bytes, max_total, settings)
            if mode == "formats":
                return download_format_report(request, opts, workspace, max_bytes, max_total, max_items, emit_progress)
            # Constructor/context configuration is local setup, never a provider phase.
            with (youtube_dl_factory or yt_dlp.YoutubeDL)(opts) as downloader:
                from network_guard import guard_failure_count
                guard_baseline = guard_failure_count()
                try:
                    with _preserve_retry_cause():
                        if ranged:
                            from clip_hls import download
                            info = downloader.extract_info(request["source_url"], download=False)
                            tracks = info.get("requested_formats") or [info]
                            sizes = [t.get("filesize") or t.get("filesize_approx") for t in tracks]
                            if all(size is not None for size in sizes) and sum(sizes) <= max_bytes:
                                return_code = downloader.download([request["source_url"]])
                            else:
                                import re
                                range_options = {**opts, "format": re.sub(r"(bestvideo|bestaudio|best)(?![a-z])", r"\1[protocol=m3u8_native]", opts["format"])}
                                with (youtube_dl_factory or yt_dlp.YoutubeDL)(range_options) as range_downloader:
                                    info = range_downloader.extract_info(request["source_url"], download=False)
                                    metadata.add(info)
                                    ranged_sources = download(info, clips, workspace, range_downloader, options, mode,
                                                              max_bytes, max_total, ffmpeg, _ffmpeg_args, _bounded_output, timeout)
                                clips = []
                                return_code = 0
                        else:
                            return_code = downloader.download([request["source_url"]])
                except MediaFailure:
                    # Metadata/quota/progress callbacks are local enforcement, not providers.
                    raise
                except Exception as exc:
                    raise _classified_provider_failure(exc, guard_baseline) from exc
                if return_code:
                    # No structured provider error survived: do not infer provider authority.
                    raise MediaFailure("source_unavailable")
        emit_progress(75.0)
        sources = sorted((p for p in workspace.iterdir() if p.is_file() and p.name.startswith("source-")), key=lambda p: p.name)
        if ranged_sources is not None:
            sources = ranged_sources
        # Finalized provider outputs in extraction order, including video of any supported type.
        if gallery:
            sources = gallery_sources
        if not sources:
            raise MediaFailure("unsupported_source")
        produced: list[Path] = []
        if gallery and mode == "image":
            sources.sort(key=lambda p: p.name)
            if options.get("gallery_zip") and len(sources) > 10:
                if max_items < 10:
                    raise MediaFailure("resource_limit")
                for item in sources[:9]:
                    produced.append(item)
                archive = workspace / "gallery.zip"
                with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
                    total = 0
                    for item in sources:
                        size = _check_size(item, max_bytes)
                        total += size
                        if total > max_total:
                            raise MediaFailure("resource_limit")
                        zipped.write(item, item.name)
                if archive.stat().st_size > max_bytes:
                    raise MediaFailure("resource_limit")
                produced.append(archive)
            else:
                produced = sources
        elif clips:
            for source_index, source in enumerate(sources, start=1):
                for clip_index, clip in enumerate(clips, start=1):
                    extension = ({"best": "m4a", "aac": "m4a", "mp3": "mp3", "opus": "opus"}[options.get("audio_codec", "best")]
                                 if mode == "audio" else options.get("container", "mp4"))
                    out = workspace / f"clip-{source_index:03d}-{clip_index:02d}.{extension}"
                    _bounded_output(_ffmpeg_args(ffmpeg, source, out, options, mode, clip), timeout, 64 * 1024)
                    produced.append(out)
                    if len(produced) > max_items:
                        raise MediaFailure("resource_limit")
        else:
            for index, source in enumerate(sources, start=1):
                if mode != "audio" and not clips:
                    info, format_name = _probe(source, ffprobe, timeout)
                    if (gallery and _media_type(info, format_name).startswith("image/")) or _already_compatible(info, format_name, options):
                        produced.append(source)
                        continue
                if mode == "audio":
                    ext = {"best": "m4a", "aac": "m4a", "mp3": "mp3", "opus": "opus"}[options.get("audio_codec", "best")]
                    target = workspace / f"media-{index:03d}.{ext}"
                else:
                    target = workspace / f"media-{index:03d}.{options.get('container', 'mp4')}"
                _bounded_output(_ffmpeg_args(ffmpeg, source, target, options, mode), timeout, 64 * 1024)
                produced.append(target)
        # Remove download intermediates from artifact eligibility, not user workspace.
        for path in sources:
            if path not in produced:
                try:
                    path.unlink()
                except OSError:
                    pass
        if len(produced) > max_items:
            raise MediaFailure("resource_limit")
        total = 0
        result = []
        for path in produced:
            size = _check_size(path, max_bytes)
            total += size
            if total > max_total:
                raise MediaFailure("resource_limit")
            if path.suffix.lower() == ".zip":
                media_type = "application/zip"
            else:
                probed, format_name = _probe(path, ffprobe, timeout)
                media_type = _media_type(probed, format_name)
            artifact = {"path": path.name, "size_bytes": size, "media_type": media_type, "metadata": metadata.finish()}
            if options.get("media_info", False) and len(produced) == 1 and path.suffix.lower() != ".zip":
                artifact["media_info"] = local_media_info(probed, size)
            result.append(artifact)
        if not result:
            raise MediaFailure("invalid_media")
        emit_progress(100.0)
        return result
    except MediaFailure:
        raise
    except Exception as exc:
        raise MediaFailure("invalid_media") from exc
    finally:
        if cookie_file:
            try:
                Path(cookie_file).unlink()
            except OSError:
                pass


def _provider_fallback(request, settings, emit_progress):
    """Attempt-local files and metadata: publish only a wholly successful provider."""
    parent = Path.cwd().resolve()
    for engine in ("yt_dlp", "gallery"):
        with tempfile.TemporaryDirectory(prefix=".provider-", dir=parent) as directory:
            attempt = Path(directory)
            local = {**request, "options": {**request.get("options", {}), "engine": engine}}
            try:
                os.chdir(attempt)
                artifacts = download_media(local, settings, emit_progress)
            except MediaFailure as exc:
                if engine != "yt_dlp" or not provider_fallback_allowed(exc):
                    raise
                continue
            finally:
                os.chdir(parent)
            for artifact in artifacts:
                shutil.move(str(attempt / artifact["path"]), parent / artifact["path"])
            return artifacts
    raise MediaFailure("source_unavailable")


@contextmanager
def _preserve_retry_cause():
    """Observe the pinned typed retry error before yt-dlp converts it to text.

    Forward the original retry behavior unchanged; only retain a causal link on the
    resulting DownloadError. This is scoped to one extractor call in one worker.
    """
    from yt_dlp.downloader.common import FileDownloader
    from yt_dlp.utils import DownloadError
    original = FileDownloader.report_retry
    def report_retry(downloader, error, *args, **kwargs):
        try:
            return original(downloader, error, *args, **kwargs)
        except DownloadError as exc:
            exc.provider_cause = error
            raise
    FileDownloader.report_retry = report_retry
    try:
        yield
    finally:
        FileDownloader.report_retry = original


def _local_callback(callback):
    def guarded(*args, **kwargs):
        try:
            return callback(*args, **kwargs)
        except MediaFailure:
            raise
        except Exception as exc:
            raise MediaFailure(classify_error(exc)) from exc
    return guarded


def _classified_provider_failure(exc, guard_baseline):
    """Explicit opt-in only for recognized provider errors from the extractor call."""
    from network_guard import guard_failure_count
    from yt_dlp.networking.exceptions import HTTPError, TransportError
    from yt_dlp.utils import ContentTooShortError, DownloadError, ExtractorError, PostProcessingError
    chain = exception_chain(exc)
    failure_class = classify_error(exc)
    if failure_class == "invalid_media" and any(isinstance(item, (ContentTooShortError, TransportError)) for item in chain):
        failure_class = "source_unavailable"
    failure = MediaFailure(failure_class)
    if len(chain) >= 64 or guard_failure_count() != guard_baseline:
        return failure
    if not isinstance(exc, (ContentTooShortError, DownloadError, ExtractorError, HTTPError, TransportError)):
        return failure
    # yt-dlp marks malformed provider JSON as unexpected despite retaining its typed cause.
    def provider_extractor_error(item):
        return isinstance(item, ExtractorError) and (
            item.expected or isinstance(item.cause, json.JSONDecodeError))

    # Unexpected implementation and local postprocessing errors remain forbidden even
    # when YoutubeDL wraps them in a DownloadError.
    if any(isinstance(item, PostProcessingError) or
           isinstance(item, ExtractorError) and not provider_extractor_error(item) for item in chain):
        return failure
    if not any(isinstance(item, (ContentTooShortError, HTTPError, TransportError)) or
               provider_extractor_error(item) for item in chain):
        return failure
    failure.fallback_forbidden = False
    if not provider_fallback_allowed(failure, original=exc):
        failure.fallback_forbidden = True
    return failure


def provider_fallback_allowed(exc, *, original=None):
    if getattr(exc, "fallback_forbidden", True) or exc.failure_class not in {
            "unsupported_source", "source_unavailable", "rate_limited", "invalid_media",
            "source_blocked", "content_unavailable"}:
        return False
    from urllib.error import HTTPError as UrllibHTTPError
    for current in exception_chain(original or exc):
        if getattr(current, "fallback_forbidden", False):
            return False
        if isinstance(current, (KeyboardInterrupt, SystemExit, PermissionError,
                                asyncio.CancelledError, concurrent.futures.CancelledError)):
            return False
        if isinstance(current, (AssertionError, AttributeError, ImportError, MemoryError,
                                NameError, RecursionError, RuntimeError, SystemError, TypeError)):
            return False
        # Local filesystem I/O is not a provider failure. Recognized transport wrappers
        # can carry connection/timeout or urllib HTTP causes, never generic local OSError.
        if isinstance(current, OSError) and not isinstance(current, (ConnectionError, TimeoutError, UrllibHTTPError)):
            return False
    return True


def local_media_info(probed, size):
    """Only closed technical fields from the already confined local ffprobe."""
    codecs = {"h264", "hevc", "av1", "vp9", "vp8", "aac", "mp3", "opus", "vorbis", "flac"}
    streams = []
    for track in probed.get("streams", [])[:16]:
        kind = track.get("codec_type")
        codec = track.get("codec_name")
        dimension = lambda key: track[key] if type(track.get(key)) is int and 0 <= track[key] <= 16384 else None
        streams.append({"kind": kind if kind in ("video", "audio", "subtitle") else "other",
                        "codec": codec if codec in codecs else "unknown",
                        "width": dimension("width"), "height": dimension("height")})
    return {"size_bytes": size, "streams": streams}
