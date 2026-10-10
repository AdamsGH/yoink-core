import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from clip_hls import _read, download, segments
from failures import MediaFailure
from media_engine import _ffmpeg_args


class ClipHlsTests(unittest.TestCase):
    def test_only_overlapping_segments_and_original_time_origin(self):
        playlist = '#EXTM3U\n#EXTINF:6,\na.ts\n#EXTINF:6,\nb.ts\n#EXTINF:6,\nc.ts\n#EXT-X-ENDLIST\n'
        self.assertEqual(segments(playlist, 'https://example.org/video/index.m3u8', 7, 13),
                         [('https://example.org/video/b.ts', 6), ('https://example.org/video/c.ts', 12)])
        self.assertEqual(segments(playlist, 'https://example.org/index.m3u8', 6, 12),
                         [('https://example.org/b.ts', 6)])

    def test_unsupported_or_incomplete_manifest_is_not_silently_misclipped(self):
        for text in ('#EXTINF:6,\na.ts\n', '#EXT-X-ENDLIST\n#EXTINF:nan,\na.ts\n',
                     '#EXT-X-ENDLIST\n#EXT-X-KEY:METHOD=AES-128\n#EXTINF:6,\na.ts\n',
                     '#EXT-X-ENDLIST\n#EXTINF:6,\na.ts\n'):
            with self.assertRaises(MediaFailure):
                segments(text, 'https://example.org/index.m3u8', 7, 13)

    def test_read_limit_checks_actual_bytes_and_closes_response(self):
        response = io.BytesIO(b'12345')
        response.url = 'https://example.org/segment.ts'
        class Downloader:
            def urlopen(self, request):
                self.request = request
                return response
        downloader = Downloader()
        with self.assertRaises(MediaFailure) as caught:
            _read(downloader, response.url, {'User-Agent': 'fixture'}, 4)
        self.assertEqual(caught.exception.failure_class, 'resource_limit')
        self.assertTrue(response.closed)
        self.assertEqual(downloader.request.headers['User-Agent'], 'fixture')

    def test_two_tracks_fetch_only_overlap_and_ffmpeg_inputs_are_local(self):
        manifest = b'#EXTM3U\n#EXTINF:6,\na.ts\n#EXTINF:6,\nb.ts\n#EXTINF:6,\nc.ts\n#EXT-X-ENDLIST\n'
        urls = []
        commands = []
        def read(downloader, url, headers, maximum):
            urls.append(url)
            return (manifest if url.endswith('.m3u8') else b'fragment'), url
        def run(command, timeout, maximum):
            commands.append(command)
            inputs = [Path(command[i + 1]) for i, value in enumerate(command) if value == '-i']
            self.assertEqual([p.read_bytes() for p in inputs], [b'fragmentfragment'] * 2)
            self.assertEqual([command[i + 1] for i, value in enumerate(command) if value == '-ss'], ['1.0', '1.0'])
            self.assertEqual(command[command.index('-t') + 1], '6.0')
            self.assertTrue(all(p.is_absolute() and p.parent == Path(command[-1]).parent for p in inputs))
            Path(command[-1]).write_bytes(b'output')
        tracks = [{'protocol': 'm3u8_native', 'url': f'https://example.org/{kind}/index.m3u8'} for kind in ('video', 'audio')]
        with tempfile.TemporaryDirectory() as directory, patch('clip_hls._read', side_effect=read):
            workspace = Path(directory)
            outputs = download({'requested_formats': tracks}, [{'start_seconds': 7, 'end_seconds': 13}],
                               workspace, object(), {}, 'video', 1024, 2048, 'ffmpeg', _ffmpeg_args, run, 10)
            self.assertEqual([p.read_bytes() for p in outputs], [b'output'])
            self.assertFalse(list(workspace.glob('.range-*')))
        self.assertEqual(len(commands), 1)
        self.assertEqual(urls, [t['url'] for t in tracks] + [f'https://example.org/{kind}/{part}.ts' for kind in ('video', 'audio') for part in ('b', 'c')])

    def test_aggregate_transfer_limit_stops_before_next_fragment(self):
        manifest = b'#EXTM3U\n#EXTINF:6,\na.ts\n#EXTINF:6,\nb.ts\n#EXT-X-ENDLIST\n'
        remaining = []
        def read(downloader, url, headers, maximum):
            if url.endswith('.m3u8'):
                return manifest, url
            remaining.append(maximum)
            return b'abcd', url
        track = {'protocol': 'm3u8_native', 'url': 'https://example.org/index.m3u8'}
        with tempfile.TemporaryDirectory() as directory, patch('clip_hls._read', side_effect=read):
            with self.assertRaises(MediaFailure) as caught:
                download(track, [{'start_seconds': 0, 'end_seconds': 12}], Path(directory), object(), {},
                         'video', 100, 4, 'ffmpeg', _ffmpeg_args, lambda *args: self.fail('must not encode'), 10)
        self.assertEqual(caught.exception.failure_class, 'resource_limit')
        self.assertEqual(remaining, [4])
