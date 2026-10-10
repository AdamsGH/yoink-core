"""Fetch VOD clip fragments through yt-dlp's guarded transport, never remote ffmpeg inputs."""
from __future__ import annotations

import math
from pathlib import Path
from urllib.parse import urljoin

from failures import MediaFailure


def segments(text: str, base: str, start: float, end: float) -> list[tuple[str, float]]:
    elapsed = 0.0
    duration = None
    selected = []
    if '#EXT-X-ENDLIST' not in text:
        raise MediaFailure('unsupported_source')
    for line in text.splitlines():
        line = line.strip()
        if line.startswith(('#EXT-X-KEY:', '#EXT-X-MAP:', '#EXT-X-BYTERANGE:', '#EXT-X-DISCONTINUITY')):
            raise MediaFailure('unsupported_source')
        if line.startswith('#EXTINF:'):
            try:
                duration = float(line.split(':', 1)[1].split(',', 1)[0])
            except ValueError as exc:
                raise MediaFailure('invalid_media') from exc
            if not math.isfinite(duration) or duration <= 0:
                raise MediaFailure('invalid_media')
        elif line and not line.startswith('#'):
            if duration is None:
                raise MediaFailure('invalid_media')
            if elapsed < end and elapsed + duration > start:
                selected.append((urljoin(base, line), elapsed))
            elapsed += duration
            duration = None
    if not selected or elapsed < end:
        raise MediaFailure('invalid_media')
    return selected


def _read(dl, url: str, headers: dict, maximum: int) -> tuple[bytes, str]:
    from yt_dlp.networking import Request
    with dl.urlopen(Request(url, headers=headers)) as response:
        data = response.read(maximum + 1)
        if len(data) > maximum:
            raise MediaFailure('resource_limit')
        return data, response.url


def download(info, clips, workspace: Path, dl, options, mode, maximum, total_maximum,
             ffmpeg, args_for, run, timeout):
    tracks = info.get('requested_formats') or [info]
    if len(tracks) not in (1, 2) or any(t.get('protocol') != 'm3u8_native' for t in tracks):
        raise MediaFailure('unsupported_source')
    manifests = []
    for track in tracks:
        body, final_url = _read(dl, track['url'], track.get('http_headers') or {}, min(maximum, 1024 * 1024))
        try:
            manifests.append((body.decode('utf-8'), final_url))
        except UnicodeDecodeError as exc:
            raise MediaFailure('invalid_media') from exc
    outputs = []
    transferred = 0
    for index, clip in enumerate(clips):
        inputs = []
        offsets = []
        for track_index, (track, manifest) in enumerate(zip(tracks, manifests)):
            chosen = segments(*manifest, clip['start_seconds'], clip['end_seconds'])
            path = workspace / f'.range-{index}-{track_index}.ts'
            size = 0
            with path.open('wb') as handle:
                for url, _ in chosen:
                    remaining = min(maximum - size, total_maximum - transferred)
                    if remaining <= 0:
                        raise MediaFailure('resource_limit')
                    body, _ = _read(dl, url, track.get('http_headers') or {}, remaining)
                    if not body:
                        raise MediaFailure('invalid_media')
                    handle.write(body)
                    size += len(body)
                    transferred += len(body)
            inputs.append(path)
            offsets.append(clip['start_seconds'] - chosen[0][1])
        extension = ({'best': 'm4a', 'aac': 'm4a', 'mp3': 'mp3', 'opus': 'opus'}[options.get('audio_codec', 'best')]
                     if mode == 'audio' else options.get('container', 'mp4'))
        output = workspace / f'source-range-{index:03d}.{extension}'
        local_clip = {'start_seconds': offsets[0], 'end_seconds': offsets[0] + clip['end_seconds'] - clip['start_seconds']}
        command = args_for(ffmpeg, inputs[0], output, options, mode, local_clip)
        if len(inputs) == 2:
            insertion = command.index('-t')
            command[insertion:insertion] = ['-ss', str(offsets[1]), '-i', str(inputs[1]), '-map', '0:v:0', '-map', '1:a:0']
        elif len(inputs) != 1:
            raise MediaFailure('unsupported_source')
        run(command, timeout, 64 * 1024)
        if output.stat().st_size > maximum:
            raise MediaFailure('resource_limit')
        outputs.append(output)
        for path in inputs:
            path.unlink()
    return outputs
