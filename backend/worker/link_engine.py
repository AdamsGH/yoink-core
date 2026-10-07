"""Bounded credential-bearing direct URLs, metadata-only, never a format report."""
from __future__ import annotations
import json
from pathlib import Path
from failures import MediaFailure, classify_error
from network_guard import validate_source_url

MAX_REPORT_BYTES = 4096

def download_links(request, base_options, workspace: Path, max_bytes, max_total, settings):
    import yt_dlp
    mode = request['task']['media_mode']
    height = request.get('options', {}).get('max_height')
    selector = ('bestaudio/best' if mode == 'audio_links' else
                f'bestvideo[height<={height}]+bestaudio/best[height<={height}]/best' if height else
                'bestvideo+bestaudio/best')
    opts = {**base_options, 'format': selector, 'skip_download': True, 'noplaylist': True,
            'playlist_items': '1', 'lazy_playlist': True, 'extract_flat': 'in_playlist',
            'match_filter': None, 'postprocessors': [], 'progress_hooks': [], 'post_hooks': [],
            'writesubtitles': False, 'writeautomaticsub': False, 'simulate': True}
    try:
        with yt_dlp.YoutubeDL(opts) as downloader:
            info = downloader.extract_info(request['source_url'], download=False)
    except MediaFailure:
        raise
    except Exception as exc:
        raise MediaFailure(classify_error(exc)) from exc
    if (not isinstance(info, dict) or info.get('_type', 'video') != 'video'
            or info.get('is_live') or info.get('live_status') in ('is_live', 'is_upcoming')
            or info.get('has_drm')):
        raise MediaFailure('unsupported_source')
    split_formats = bool(info.get('requested_formats'))
    formats = info.get('requested_formats') or [info]
    if not isinstance(formats, list) or len(formats) > 2:
        raise MediaFailure('unsupported_source')
    result = {'video_url': None, 'audio_url': None}
    for fmt in formats:
        if not isinstance(fmt, dict) or fmt.get('has_drm'):
            raise MediaFailure('unsupported_source')
        value = fmt.get('url')
        if (not isinstance(value, str) or not value or len(value) > 1800 or not value.isascii()
                or any(ord(c) <= 32 or ord(c) == 127 for c in value)):
            raise MediaFailure('invalid_media')
        # Apply the existing egress policy to extracted URLs too; never probe/download them.
        validate_source_url(value, settings.allowed_private_origins)
        if fmt.get('vcodec') not in (None, 'none') and mode != 'audio_links':
            result['video_url'] = value
        elif fmt.get('acodec') not in (None, 'none') or not split_formats:
            result['audio_url'] = value
    if not any(result.values()):
        raise MediaFailure('unsupported_source')
    encoded = json.dumps(result, separators=(',', ':')).encode('ascii')
    if len(encoded) > min(MAX_REPORT_BYTES, max_bytes, max_total):
        raise MediaFailure('resource_limit')
    path = workspace / 'links.json'
    with path.open('xb') as file:
        file.write(encoded)
    return [{'path': 'links.json', 'media_type': 'application/json', 'size_bytes': len(encoded)}]
