import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import yt_dlp
from link_engine import download_links
from main import _validate_request
from media_engine import local_media_info
from failures import MediaFailure

class LinksTests(unittest.TestCase):
    def request(self, mode='links', height=None):
        return {'version':2, 'operation_id':'11111111-1111-1111-1111-111111111111', 'attempt':1,
                'source_url':'https://example.com/source', 'task':{'kind':'download','media_mode':mode},
                'options':{'max_height':height}}

    def run_links(self, info, mode='links', height=None):
        request = _validate_request(json.dumps(self.request(mode, height)).encode())
        seen = {}
        class Downloader:
            def __init__(self, opts): seen.update(opts)
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def extract_info(self, url, download):
                assert download is False
                return info
            def download(self, *_): raise AssertionError('must not download')
        with tempfile.TemporaryDirectory() as directory, patch.object(yt_dlp,'YoutubeDL',Downloader), patch('link_engine.validate_source_url') as guard:
            result=download_links(request, {}, Path(directory),4096,4096,SimpleNamespace(allowed_private_origins=()))
            self.assertEqual(result[0]['path'],'links.json')
            output=json.loads((Path(directory)/'links.json').read_bytes())
            self.assertGreater(guard.call_count,0)
            return output,seen

    def test_separate_video_audio_urls_bounded_metadata_only(self):
        output,opts=self.run_links({'requested_formats':[
            {'vcodec':'h264','acodec':'none','url':'https://cdn.example/video?signature=fixture'},
            {'vcodec':'none','acodec':'aac','url':'https://cdn.example/audio'}]},height=720)
        self.assertIn('signature=fixture',output['video_url'])
        self.assertEqual(output['audio_url'],'https://cdn.example/audio')
        self.assertTrue(opts['skip_download'])
        self.assertEqual(opts['postprocessors'],[])
        self.assertIn('height<=720',opts['format'])

    def test_audio_and_combined_source(self):
        output,opts=self.run_links({'vcodec':'none','acodec':'opus','url':'https://cdn.example/audio'},'audio_links')
        self.assertIsNone(output['video_url'])
        self.assertEqual(opts['format'],'bestaudio/best')
        output,_=self.run_links({'vcodec':'h264','acodec':'aac','url':'https://cdn.example/combined'})
        self.assertIsNone(output['audio_url'])

    def test_live_playlist_drm_oversize_and_invalid_options_rejected(self):
        for info in [{'_type':'playlist'}, {'is_live':True}, {'has_drm':True},
                     {'vcodec':'h264','url':'https://cdn.example/'+('x'*1800)}]:
            with self.subTest(info=info), self.assertRaises(MediaFailure): self.run_links(info)
        for mode,height in [('links',2160),('audio_links',720)]:
            with self.assertRaises(ValueError): _validate_request(json.dumps(self.request(mode,height)).encode())

    def test_local_report_never_carries_probe_tags_or_arbitrary_prose(self):
        report=local_media_info({'streams':[{'codec_type':'video','codec_name':'h264','width':1280,'height':720,
                                            'tags':{'secret':'credentials'},'filename':'/private/file'}]}, 123)
        self.assertEqual(report,{'size_bytes':123,'streams':[{'kind':'video','codec':'h264','width':1280,'height':720}]})
        self.assertNotIn('secret',json.dumps(report))

class DirectLinksContainerFixture(unittest.TestCase):
    def test_real_generic_extractor_returns_url_without_media_body_download(self):
        import http.server
        import threading
        from network_guard import guard_context
        requests = []
        class Source(http.server.BaseHTTPRequestHandler):
            def do_HEAD(self):
                requests.append(('HEAD', self.path))
                self.send_response(200)
                self.send_header('Content-Type','video/mp4')
                self.send_header('Content-Length','1048576')
                self.end_headers()
            def do_GET(self):
                requests.append(('GET', self.path))
                self.send_response(200)
                self.send_header('Content-Type','video/mp4')
                self.send_header('Content-Length','1048576')
                self.end_headers()
                # Generic only reads a small sniff; no actual complete media exists.
                try: self.wfile.write(b'\x00\x00\x00\x18ftypisom0000'+b'\x00'*1000)
                except BrokenPipeError: pass
            def log_message(self,*args): pass
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Source)
        thread=threading.Thread(target=server.serve_forever,daemon=True); thread.start()
        origin=f'http://127.0.0.1:{server.server_port}'
        settings=SimpleNamespace(allowed_private_origins=(origin,))
        request={'source_url':origin+'/fixture.mp4?signature=fixture','task':{'kind':'download','media_mode':'links'},'options':{}}
        try:
            with tempfile.TemporaryDirectory() as directory, guard_context((origin,)):
                result=download_links(request, {'quiet':True,'socket_timeout':3,'retries':0}, Path(directory),4096,4096,settings)
                output=json.loads((Path(directory)/'links.json').read_bytes())
                self.assertIn(origin+'/fixture.mp4?signature=fixture',output.values())
                self.assertEqual([p.name for p in Path(directory).iterdir()],['links.json'])
                self.assertEqual(result[0]['media_type'],'application/json')
        finally:
            server.shutdown(); server.server_close(); thread.join()
        self.assertLessEqual(len(requests),2)
